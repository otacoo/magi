//! GB/GBC netplay.
//!
//! The connection half is core-agnostic: host/join roles, a four-character
//! code, UDP discovery, hotspot or shared Wi-Fi, a TCP handshake and session
//! lifetime. The payload here is gambatte's two-byte Game Link frame. A later
//! adapter can reuse the connection half for a state-sync core.
//!
//! Gambatte's serial link is a byte pipe: the core connects to a TCP socket
//! and every 8-bit link transfer becomes a two-byte frame. MAGI owns that
//! socket through a local proxy, and the proxy carries the frames to the other
//! console. Each device runs one game with its own save.

use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Port the host listens on for the other console.
pub const PEER_PORT: u16 = 7211;
/// UDP port used to advertise a host on a shared network.
pub const DISCOVERY_PORT: u16 = 7212;
/// Local port the core's netlink client connects to.
pub const LOCAL_PORT: u16 = 56400;

#[cfg(target_os = "linux")]
const HOTSPOT_PREFIX: &str = "MAGI-";
#[cfg(target_os = "linux")]
const HOTSPOT_PSK: &str = "magilink0";
#[cfg(target_os = "linux")]
const HOTSPOT_HOST: &str = "10.42.0.1";
#[cfg(target_os = "linux")]
const HOTSPOT_PEER: &str = "10.42.0.2";
const MAGIC: &[u8; 4] = b"MAGI";
const VERSION: u8 = 1;
const SCAN_TIME: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(8);
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Hotspot,
    Wifi,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Hotspot => "HOTSPOT",
            Transport::Wifi => "WIFI",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Host,
    Join,
}

#[derive(Clone, Debug)]
pub struct Game {
    pub system: String,
    pub title: String,
    pub code: String,
}

impl Game {
    pub fn from_rom(system: &str, rom: &str) -> Game {
        let title = Path::new(rom)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("GAME")
            .to_string();
        let mut code = String::new();
        if let Ok(bytes) = std::fs::read(rom) {
            let header = bytes.get(0x134..0x144).unwrap_or(&[]);
            if header.len() == 16 {
                let cgb = header[0x143 - 0x134] & 0x80 != 0;
                let from = if cgb { 0x13F - 0x134 } else { 0 };
                for &b in &header[from..from + 4] {
                    if b.is_ascii_alphanumeric() {
                        code.push((b as char).to_ascii_uppercase());
                    }
                }
            }
        }
        if code.len() != 4 {
            code = title
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(4)
                .collect::<String>()
                .to_ascii_uppercase();
        }
        if code.len() != 4 {
            code = "GB".into();
        }
        Game {
            system: system.to_string(),
            title,
            code,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub code: String,
    pub addr: IpAddr,
    pub port: u16,
    pub title: String,
    pub hotspot: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Waiting,
    Scanning,
    Found,
    Connecting,
    Linked,
    Lost,
    Error(String),
    Stopped,
}

type SharedStatus = Arc<Mutex<Status>>;
type SharedHosts = Arc<Mutex<Vec<HostInfo>>>;
type SharedSockets = Arc<Mutex<Vec<TcpStream>>>;

pub struct Session {
    role: Role,
    transport: Transport,
    code: String,
    game: Game,
    status: SharedStatus,
    hosts: SharedHosts,
    cancel: Arc<AtomicBool>,
    sockets: SharedSockets,
    radio: Arc<Mutex<Option<radio::Radio>>>,
    local_port: u16,
    worker: Option<JoinHandle<()>>,
}

impl Session {
    /// Start advertising and waiting for a joiner.
    pub fn host(transport: Transport, game: Game) -> Session {
        Self::host_with(transport, game, LOCAL_PORT, false)
    }

    fn host_with(transport: Transport, game: Game, local_port: u16, loopback: bool) -> Session {
        let code = new_code();
        crate::util::log(&format!(
            "netplay: hosting {} code={code}",
            transport.label()
        ));
        let status = Arc::new(Mutex::new(Status::Starting));
        let hosts = Arc::new(Mutex::new(Vec::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let radio = Arc::new(Mutex::new(None));
        let worker = {
            let game = game.clone();
            let code = code.clone();
            let status = status.clone();
            let cancel = cancel.clone();
            let sockets = sockets.clone();
            let radio = radio.clone();
            thread::spawn(move || {
                host_worker(
                    transport, game, code, local_port, loopback, status, cancel, sockets, radio,
                );
            })
        };
        Session {
            role: Role::Host,
            transport,
            code,
            game,
            status,
            hosts,
            cancel,
            sockets,
            radio,
            local_port,
            worker: Some(worker),
        }
    }

    /// Scan for hosts on the selected transport.
    pub fn join(transport: Transport, game: Game) -> Session {
        Self::join_with(transport, game, LOCAL_PORT, false)
    }

    fn join_with(transport: Transport, game: Game, local_port: u16, loopback: bool) -> Session {
        crate::util::log(&format!("netplay: scanning {}", transport.label()));
        let status = Arc::new(Mutex::new(Status::Scanning));
        let hosts = Arc::new(Mutex::new(Vec::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let radio = Arc::new(Mutex::new(None));
        let worker = {
            let status = status.clone();
            let hosts = hosts.clone();
            let cancel = cancel.clone();
            thread::spawn(move || {
                join_scan_worker(transport, loopback, status, hosts, cancel);
            })
        };
        Session {
            role: Role::Join,
            transport,
            code: String::new(),
            game,
            status,
            hosts,
            cancel,
            sockets,
            radio,
            local_port,
            worker: Some(worker),
        }
    }

    #[cfg(test)]
    fn host_local(transport: Transport, game: Game, local_port: u16) -> Session {
        Self::host_with(transport, game, local_port, true)
    }

    #[cfg(test)]
    fn join_local(transport: Transport, game: Game, local_port: u16) -> Session {
        Self::join_with(transport, game, local_port, true)
    }

    /// Connect to one of the hosts found by `join`.
    pub fn connect(&mut self, host: HostInfo) {
        crate::util::log(&format!(
            "netplay: joining code={} at {}:{}",
            host.code, host.addr, host.port
        ));
        if let Some(old) = self.worker.take() {
            let _ = old; // the scan worker has finished
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        self.code = host.code.clone();
        set_status(&self.status, Status::Connecting);
        let status = self.status.clone();
        let cancel = self.cancel.clone();
        let sockets = self.sockets.clone();
        let radio = self.radio.clone();
        let game = self.game.clone();
        let transport = self.transport;
        let local_port = self.local_port;
        self.worker = Some(thread::spawn(move || {
            connect_worker(
                transport, game, host, local_port, status, cancel, sockets, radio,
            );
        }));
    }

    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    pub fn hosts(&self) -> Vec<HostInfo> {
        lock(&self.hosts).clone()
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        for s in lock(&self.sockets).iter() {
            let _ = s.shutdown(Shutdown::Both);
        }
        if let Some(handle) = self.worker.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        if let Some(mut r) = lock(&self.radio).take() {
            radio::stop(&mut r);
        }
        set_status(&self.status, Status::Stopped);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_status(status: &SharedStatus, value: Status) {
    *lock(status) = value;
}

fn fail(status: &SharedStatus, cancel: &AtomicBool, msg: impl Into<String>) {
    if !cancel.load(Ordering::Relaxed) {
        set_status(status, Status::Error(msg.into()));
    }
}

fn host_worker(
    transport: Transport,
    game: Game,
    code: String,
    local_port: u16,
    loopback: bool,
    status: SharedStatus,
    cancel: Arc<AtomicBool>,
    sockets: SharedSockets,
    radio_slot: Arc<Mutex<Option<radio::Radio>>>,
) {
    if transport == Transport::Hotspot {
        let mut radio = match radio::host_start(&code) {
            Ok(r) => r,
            Err(e) => {
                fail(&status, &cancel, e);
                return;
            }
        };
        if cancel.load(Ordering::Relaxed) {
            radio::stop(&mut radio);
            return;
        }
        *lock(&radio_slot) = Some(radio);
    } else if !loopback {
        if let Err(e) = ensure_wifi(&cancel) {
            fail(&status, &cancel, e);
            return;
        }
    }

    crate::util::log(&format!("netplay: host {} code={code}", transport.label()));
    let bind = if loopback { "127.0.0.1" } else { "0.0.0.0" };
    let listener = match TcpListener::bind((bind, PEER_PORT)) {
        Ok(l) => l,
        Err(e) => {
            fail(&status, &cancel, format!("PORT {PEER_PORT}: {e}"));
            stop_radio(&radio_slot);
            return;
        }
    };
    if let Err(e) = listener.set_nonblocking(true) {
        fail(&status, &cancel, format!("PORT {PEER_PORT}: {e}"));
        stop_radio(&radio_slot);
        return;
    }
    set_status(&status, Status::Waiting);

    let advertise = Arc::new(AtomicBool::new(false));
    let advertiser = {
        let game = game.clone();
        let code = code.clone();
        let stop = advertise.clone();
        thread::spawn(move || advertise_host(&game, &code, &stop, loopback))
    };

    let mut peer = None;
    while !cancel.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(HELLO_TIMEOUT));
                match read_hello(&mut stream) {
                    Ok(hello) if hello.code == code => {
                        if write_ack(&mut stream, 0, &code).is_ok() {
                            crate::util::log(&format!("netplay: peer joined code={code}"));
                            peer = Some(stream);
                            break;
                        }
                    }
                    _ => {
                        let _ = write_ack(&mut stream, 2, &code);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                fail(&status, &cancel, format!("LINK PORT: {e}"));
                advertise.store(true, Ordering::Relaxed);
                stop_radio(&radio_slot);
                return;
            }
        }
    }
    advertise.store(true, Ordering::Relaxed);
    let _ = advertiser.join();
    let Some(peer) = peer else {
        stop_radio(&radio_slot);
        return;
    };
    let _ = peer.set_read_timeout(None);
    let _ = peer.set_write_timeout(None);
    start_link(peer, local_port, status, cancel, sockets, radio_slot);
}

fn join_scan_worker(
    transport: Transport,
    loopback: bool,
    status: SharedStatus,
    hosts: SharedHosts,
    cancel: Arc<AtomicBool>,
) {
    let result = match transport {
        Transport::Wifi => scan_wifi(&status, &hosts, &cancel, loopback),
        Transport::Hotspot => scan_hotspot(&status, &hosts, &cancel),
    };
    if let Err(e) = result {
        fail(&status, &cancel, e);
        return;
    }
    if !cancel.load(Ordering::Relaxed) {
        set_status(&status, Status::Found);
    }
}

fn connect_worker(
    transport: Transport,
    game: Game,
    host: HostInfo,
    local_port: u16,
    status: SharedStatus,
    cancel: Arc<AtomicBool>,
    sockets: SharedSockets,
    radio_slot: Arc<Mutex<Option<radio::Radio>>>,
) {
    if transport == Transport::Hotspot {
        if host.code.is_empty() {
            fail(&status, &cancel, "NO HOST CODE");
            return;
        }
        match radio::join_start(&host.code) {
            Ok(r) => *lock(&radio_slot) = Some(r),
            Err(e) => {
                fail(&status, &cancel, e);
                return;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            stop_radio(&radio_slot);
            return;
        }
    }

    let addr: std::net::SocketAddr = (host.addr, host.port).into();
    let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(8)) {
        Ok(s) => s,
        Err(e) => {
            stop_radio(&radio_slot);
            fail(&status, &cancel, format!("CONNECT {addr}: {e}"));
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HELLO_TIMEOUT));
    let mut stream = stream;
    if let Err(e) = write_hello(&mut stream, &host.code, &game) {
        stop_radio(&radio_slot);
        fail(&status, &cancel, format!("HELLO: {e}"));
        return;
    }
    match read_ack(&mut stream) {
        Ok((0, code)) if code == host.code => {}
        Ok((code, _)) => {
            stop_radio(&radio_slot);
            fail(&status, &cancel, ack_error(code));
            return;
        }
        Err(e) => {
            stop_radio(&radio_slot);
            fail(&status, &cancel, format!("HOST: {e}"));
            return;
        }
    }
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);
    start_link(stream, local_port, status, cancel, sockets, radio_slot);
}

fn ack_error(code: u8) -> String {
    match code {
        1 => "HOST IS BUSY".into(),
        2 => "WRONG CODE".into(),
        _ => "HOST REFUSED".into(),
    }
}

/// The peer is connected. Open the local core port, report Linked, and relay
/// the core's two-byte serial frames to the peer.
fn start_link(
    peer: TcpStream,
    local_port: u16,
    status: SharedStatus,
    cancel: Arc<AtomicBool>,
    sockets: SharedSockets,
    radio_slot: Arc<Mutex<Option<radio::Radio>>>,
) {
    let local = match TcpListener::bind(("127.0.0.1", local_port)) {
        Ok(l) => l,
        Err(e) => {
            stop_radio(&radio_slot);
            fail(&status, &cancel, format!("LOCAL PORT: {e}"));
            return;
        }
    };
    if let Err(e) = local.set_nonblocking(true) {
        stop_radio(&radio_slot);
        fail(&status, &cancel, format!("LOCAL PORT: {e}"));
        return;
    }
    set_status(&status, Status::Linked);
    let _ = peer.set_nodelay(true);
    set_keepalive(&peer);
    crate::util::log(&format!("netplay: linked, core port {local_port}"));
    let _ = peer.set_read_timeout(None);
    let _ = peer.set_write_timeout(None);
    if let Ok(clone) = peer.try_clone() {
        lock(&sockets).push(clone);
    }
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let core = loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            match local.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        set_status(&status, Status::Error("CORE LINK DID NOT START".into()));
                        let _ = peer.shutdown(Shutdown::Both);
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    set_status(&status, Status::Error(format!("LOCAL LINK: {e}")));
                    let _ = peer.shutdown(Shutdown::Both);
                    return;
                }
            }
        };
        let _ = core.set_nodelay(true);
        if let Ok(clone) = core.try_clone() {
            lock(&sockets).push(clone);
        }
        relay(core, peer, status, cancel, sockets);
    });
}

/// Two independent two-byte relays, one per direction.
fn relay(
    core: TcpStream,
    peer: TcpStream,
    status: SharedStatus,
    cancel: Arc<AtomicBool>,
    sockets: SharedSockets,
) {
    let (core_r, core_w, peer_r, peer_w) = match (
        core.try_clone(),
        core.try_clone(),
        peer.try_clone(),
        peer.try_clone(),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        _ => {
            set_status(&status, Status::Error("LINK SOCKET: clone failed".into()));
            return;
        }
    };
    let a = {
        let status = status.clone();
        let cancel = cancel.clone();
        let sockets = sockets.clone();
        thread::spawn(move || pump(core_r, peer_w, status, cancel, sockets))
    };
    let b = {
        let status = status.clone();
        let cancel = cancel.clone();
        let sockets = sockets.clone();
        thread::spawn(move || pump(peer_r, core_w, status, cancel, sockets))
    };
    let _ = a;
    let _ = b;
}

fn pump(
    mut from: TcpStream,
    mut to: TcpStream,
    status: SharedStatus,
    cancel: Arc<AtomicBool>,
    sockets: SharedSockets,
) {
    let mut frame = [0u8; 2];
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if from.read_exact(&mut frame).is_err() {
            break;
        }
        if to.write_all(&frame).is_err() {
            break;
        }
    }
    if !cancel.load(Ordering::Relaxed) {
        crate::util::log("netplay: link lost");
        set_status(&status, Status::Lost);
    }
    for s in lock(&sockets).iter() {
        let _ = s.shutdown(Shutdown::Both);
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}

fn stop_radio(slot: &Arc<Mutex<Option<radio::Radio>>>) {
    if let Some(mut r) = lock(slot).take() {
        radio::stop(&mut r);
    }
}

#[cfg(target_os = "linux")]
fn set_keepalive(stream: &TcpStream) {
    use std::os::unix::io::AsRawFd;

    let fd = stream.as_raw_fd();
    let on: libc::c_int = 1;
    let idle: libc::c_int = 10;
    let interval: libc::c_int = 5;
    let count: libc::c_int = 3;
    // SAFETY: fd is a live TCP socket; each option points at a c_int of the
    // length given. Failures only mean the peer is not probed.
    unsafe {
        let set = |level, name, value: &libc::c_int| {
            libc::setsockopt(
                fd,
                level,
                name,
                value as *const _ as *const libc::c_void,
                std::mem::size_of_val(value) as libc::socklen_t,
            );
        };
        set(libc::SOL_SOCKET, libc::SO_KEEPALIVE, &on);
        set(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, &idle);
        set(libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, &interval);
        set(libc::IPPROTO_TCP, libc::TCP_KEEPCNT, &count);
    }
}

#[cfg(not(target_os = "linux"))]
fn set_keepalive(_stream: &TcpStream) {}

fn advertise_host(game: &Game, code: &str, stop: &AtomicBool, loopback: bool) {
    let bind = if loopback { "127.0.0.1" } else { "0.0.0.0" };
    let sock = match UdpSocket::bind((bind, 0)) {
        Ok(sock) => sock,
        Err(e) => {
            crate::util::log(&format!("netplay: advertise socket: {e}"));
            return;
        }
    };
    #[cfg(target_os = "linux")]
    if !loopback {
        let _ = sock.set_broadcast(true);
    }
    let _ = sock.set_nonblocking(true);
    let line = discovery_line(game, code);
    crate::util::log(&format!("netplay: advertising code={code}"));
    while !stop.load(Ordering::Relaxed) {
        let _ = sock.send_to(line.as_bytes(), ("127.0.0.1", DISCOVERY_PORT));
        #[cfg(target_os = "linux")]
        if !loopback {
            let _ = sock.send_to(line.as_bytes(), ("255.255.255.255", DISCOVERY_PORT));
        }
        for _ in 0..14 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn bind_discovery(loopback: bool) -> Result<UdpSocket, String> {
    let bind = if loopback { "127.0.0.1" } else { "0.0.0.0" };
    let mut last = String::new();
    for _ in 0..5 {
        match UdpSocket::bind((bind, DISCOVERY_PORT)) {
            Ok(sock) => return Ok(sock),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                last = format!("DISCOVERY PORT {DISCOVERY_PORT}: {e}");
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("DISCOVERY PORT {DISCOVERY_PORT}: {e}")),
        }
    }
    Err(last)
}

#[cfg(target_os = "linux")]
fn ensure_wifi(cancel: &AtomicBool) -> Result<(), String> {
    if !crate::util::wifi_up() {
        crate::util::set_wifi(true);
    }
    for _ in 0..40 {
        if cancel.load(Ordering::Relaxed) {
            return Err("CANCELLED".into());
        }
        if crate::util::wifi_has_ip() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("WI-FI HAS NO ADDRESS".into())
}

#[cfg(not(target_os = "linux"))]
fn ensure_wifi(_cancel: &AtomicBool) -> Result<(), String> {
    Ok(())
}

fn scan_wifi(
    _status: &SharedStatus,
    hosts: &SharedHosts,
    cancel: &AtomicBool,
    loopback: bool,
) -> Result<(), String> {
    if !loopback {
        ensure_wifi(cancel)?;
    }
    let sock = bind_discovery(loopback)?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| format!("DISCOVERY: {e}"))?;
    let deadline = Instant::now() + SCAN_TIME;
    let mut buf = [0u8; 512];
    let mut found: Vec<HostInfo> = Vec::new();
    while Instant::now() < deadline && !cancel.load(Ordering::Relaxed) {
        match sock.recv_from(&mut buf) {
            Ok((n, addr)) => {
                if let Some(host) = parse_discovery(&buf[..n], addr.ip()) {
                    if !found
                        .iter()
                        .any(|h| h.code == host.code && h.addr == host.addr)
                    {
                        crate::util::log(&format!("netplay: found {} at {}", host.code, host.addr));
                        found.push(host);
                        *lock(hosts) = found.clone();
                        return Ok(());
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(format!("DISCOVERY: {e}")),
        }
    }
    Ok(())
}

fn scan_hotspot(
    _status: &SharedStatus,
    hosts: &SharedHosts,
    cancel: &AtomicBool,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if !crate::util::wifi_up() {
            crate::util::set_wifi(true);
        }
        for _ in 0..12 {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let nets = crate::util::wifi_scan();
            let mut found = Vec::new();
            for net in nets {
                let Some(code) = net.ssid.strip_prefix(HOTSPOT_PREFIX) else {
                    continue;
                };
                let code = code.trim().to_ascii_uppercase();
                if code.len() == 4 && !found.iter().any(|h: &HostInfo| h.code == code) {
                    found.push(HostInfo {
                        code,
                        addr: HOTSPOT_HOST.parse().unwrap(),
                        port: PEER_PORT,
                        title: "HOTSPOT".into(),
                        hotspot: true,
                    });
                }
            }
            if !found.is_empty() {
                *lock(hosts) = found;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(700));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (_status, hosts, cancel);
        Err("HOTSPOT NEEDS THE DEVICE".into())
    }
}

fn discovery_line(game: &Game, code: &str) -> String {
    let system = if game.system.eq_ignore_ascii_case("gbc") {
        "GBC"
    } else {
        "GB"
    };
    let title = game.title.replace(['\n', '\r'], " ");
    format!(
        "MAGI1 {code} {PEER_PORT} {system} {} {}\n",
        game.code,
        title.chars().take(48).collect::<String>()
    )
}

fn parse_discovery(buf: &[u8], addr: IpAddr) -> Option<HostInfo> {
    let text = std::str::from_utf8(buf).ok()?;
    let mut parts = text.trim().splitn(6, ' ');
    if parts.next()? != "MAGI1" {
        return None;
    }
    let code = parts.next()?.to_ascii_uppercase();
    let port: u16 = parts.next()?.parse().ok()?;
    let _system = parts.next()?;
    let _game = parts.next()?;
    let title = parts.next().unwrap_or("GAME");
    if code.len() != 4 {
        return None;
    }
    Some(HostInfo {
        code,
        addr,
        port,
        title: title.to_string(),
        hotspot: false,
    })
}

struct Hello {
    code: String,
    _game: Game,
}

fn write_hello(w: &mut impl Write, code: &str, game: &Game) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(code.as_bytes());
    out.push(if game.system.eq_ignore_ascii_case("gbc") {
        1
    } else {
        0
    });
    out.extend_from_slice(game.code.as_bytes().get(..4).unwrap_or(&b"GB  "[..]));
    let title = game.title.replace(['\n', '\r'], " ");
    let title = title.as_bytes();
    let n = title.len().min(48);
    out.push(n as u8);
    out.extend_from_slice(&title[..n]);
    w.write_all(&out)?;
    w.flush()
}

fn read_hello(r: &mut impl Read) -> std::io::Result<Hello> {
    let mut head = [0u8; 10];
    r.read_exact(&mut head)?;
    if &head[..4] != MAGIC || head[4] != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not MAGI netplay",
        ));
    }
    let code = String::from_utf8_lossy(&head[5..9]).to_string();
    let system = if head[9] == 1 { "gbc" } else { "gb" };
    let mut game_code = [0u8; 4];
    r.read_exact(&mut game_code)?;
    let mut len = [0u8; 1];
    r.read_exact(&mut len)?;
    let mut title = vec![0u8; len[0] as usize];
    r.read_exact(&mut title)?;
    Ok(Hello {
        code,
        _game: Game {
            system: system.into(),
            title: String::from_utf8_lossy(&title).to_string(),
            code: String::from_utf8_lossy(&game_code).to_string(),
        },
    })
}

fn write_ack(w: &mut impl Write, code: u8, host: &str) -> std::io::Result<()> {
    let mut out = [0u8; 10];
    out[..4].copy_from_slice(MAGIC);
    out[4] = VERSION;
    out[5] = code;
    out[6..].copy_from_slice(host.as_bytes().get(..4).unwrap_or(&b"    "[..]));
    w.write_all(&out)?;
    w.flush()
}

fn read_ack(r: &mut impl Read) -> std::io::Result<(u8, String)> {
    let mut head = [0u8; 10];
    r.read_exact(&mut head)?;
    if &head[..4] != MAGIC || head[4] != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad host reply",
        ));
    }
    Ok((head[5], String::from_utf8_lossy(&head[6..]).to_string()))
}

fn new_code() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos ^ ((std::process::id() as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
    let mut out = String::with_capacity(4);
    for _ in 0..4 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.push(CODE_CHARS[(seed as usize) % CODE_CHARS.len()] as char);
    }
    out
}

#[cfg(target_os = "linux")]
mod radio {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::{HOTSPOT_PEER, HOTSPOT_PREFIX, HOTSPOT_PSK};
    use crate::util;

    const AP_CONF: &str = "/tmp/magi-netplay-ap.conf";
    const AP_CTRL: &str = "/tmp/magi-netplay-ap";

    pub enum Radio {
        Host,
        Join { ssid: String },
    }

    pub fn host_start(code: &str) -> Result<Radio, String> {
        let ssid = format!("{HOTSPOT_PREFIX}{code}");
        let conf = format!(
            "ctrl_interface={AP_CTRL}\nap_scan=2\nnetwork={{\n\
             \tssid=\"{ssid}\"\n\tmode=2\n\tfrequency=2412\n\tkey_mgmt=WPA-PSK\n\
             \tproto=RSN\n\tpairwise=CCMP\n\tpsk=\"{HOTSPOT_PSK}\"\n}}\n"
        );
        let _ = std::fs::remove_dir_all(AP_CTRL);
        let _ = std::fs::create_dir_all(AP_CTRL);
        if let Err(e) = std::fs::write(AP_CONF, conf) {
            return Err(format!("HOTSPOT CONFIG: {e}"));
        }
        let _ = Command::new("wpa_cli")
            .args(["-i", "wlan0", "terminate"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        stop_supplicant();
        let _ = Command::new("rfkill").args(["unblock", "wifi"]).status();
        let _ = Command::new("ip")
            .args(["link", "set", "wlan0", "up"])
            .status();
        let _ = Command::new("ip")
            .args(["addr", "flush", "dev", "wlan0"])
            .status();
        let out = Command::new("wpa_supplicant")
            .args(["-B", "-i", "wlan0", "-c", AP_CONF, "-Dnl80211"])
            .output()
            .map_err(|e| format!("HOTSPOT: {e}"))?;
        if !out.status.success() {
            restore();
            return Err("HOTSPOT: RADIO REFUSED AP MODE".into());
        }
        let mut ready = false;
        for _ in 0..50 {
            let out = Command::new("wpa_cli")
                .args(["-p", AP_CTRL, "-i", "wlan0", "status"])
                .output();
            if out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("wpa_state=COMPLETED"))
            {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !ready {
            restore();
            return Err("HOTSPOT: RADIO DID NOT COME UP".into());
        }
        let _ = Command::new("ip")
            .args(["addr", "add", "10.42.0.1/24", "dev", "wlan0"])
            .status();
        Ok(Radio::Host)
    }

    pub fn join_start(code: &str) -> Result<Radio, String> {
        let ssid = format!("{HOTSPOT_PREFIX}{code}");
        if !util::wifi_up() {
            util::set_wifi(true);
        }
        if !util::wifi_connect(&ssid, Some(HOTSPOT_PSK)) {
            return Err("HOTSPOT: CONNECT FAILED".into());
        }
        let mut joined = false;
        for _ in 0..200 {
            let (up, connected, current, _) = util::wifi_state();
            if up && connected && current == ssid {
                joined = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !joined {
            util::wifi_forget(&ssid);
            restore_station();
            return Err("HOTSPOT: NO ANSWER".into());
        }
        util::kill_by_name("udhcpc");
        let _ = Command::new("ip")
            .args(["addr", "flush", "dev", "wlan0"])
            .status();
        let peer = format!("{HOTSPOT_PEER}/24");
        let out = Command::new("ip")
            .args(["addr", "add", peer.as_str(), "dev", "wlan0"])
            .output()
            .map_err(|e| format!("HOTSPOT ADDRESS: {e}"))?;
        if !out.status.success() {
            util::wifi_forget(&ssid);
            restore_station();
            return Err("HOTSPOT ADDRESS FAILED".into());
        }
        Ok(Radio::Join { ssid })
    }

    pub fn stop(radio: &mut Radio) {
        match radio {
            Radio::Host => restore(),
            Radio::Join { ssid } => {
                util::wifi_forget(ssid);
                restore_station();
            }
        }
    }

    fn stop_supplicant() {
        util::kill_by_name("wpa_supplicant");
        for _ in 0..20 {
            if !util::process_running("wpa_supplicant") {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn restore_station() {
        let _ = Command::new("wpa_cli")
            .args(["-i", "wlan0", "reconnect"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        util::set_wifi(true);
    }

    fn restore() {
        let _ = Command::new("wpa_cli")
            .args(["-p", AP_CTRL, "-i", "wlan0", "terminate"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        stop_supplicant();
        let _ = Command::new("ip")
            .args(["addr", "flush", "dev", "wlan0"])
            .status();
        util::set_wifi(true);
        let _ = std::fs::remove_file(AP_CONF);
        let _ = std::fs::remove_dir_all(AP_CTRL);
    }
}

#[cfg(not(target_os = "linux"))]
mod radio {
    pub enum Radio {}

    pub fn host_start(_code: &str) -> Result<Radio, String> {
        Err("HOTSPOT NEEDS THE DEVICE".into())
    }

    pub fn join_start(_code: &str) -> Result<Radio, String> {
        Err("HOTSPOT NEEDS THE DEVICE".into())
    }

    pub fn stop(_radio: &mut Radio) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn codes_are_four_safe_characters() {
        for _ in 0..64 {
            let code = new_code();
            assert_eq!(code.len(), 4);
            assert!(code.bytes().all(|b| CODE_CHARS.contains(&b)));
        }
    }

    #[test]
    fn discovery_lines_round_trip() {
        let game = Game {
            system: "gbc".into(),
            title: "Tetris (World)".into(),
            code: "TETR".into(),
        };
        let line = discovery_line(&game, "3B7P");
        let host = parse_discovery(line.as_bytes(), "10.0.0.2".parse().unwrap()).unwrap();
        assert_eq!(host.code, "3B7P");
        assert_eq!(host.port, PEER_PORT);
        assert_eq!(host.title, "Tetris (World)");
    }

    #[test]
    fn hello_round_trips() {
        let (mut a, mut b) = pipe();
        let game = Game {
            system: "gb".into(),
            title: "Tetris".into(),
            code: "TETR".into(),
        };
        write_hello(&mut a, "3B7P", &game).unwrap();
        let hello = read_hello(&mut b).unwrap();
        assert_eq!(hello.code, "3B7P");
        assert_eq!(hello._game.title, "Tetris");
    }

    #[test]
    fn two_byte_frames_cross_the_proxy() {
        let (core_a, mut core_b) = pipe();
        let (mut peer_a, peer_b) = pipe();
        let status = Arc::new(Mutex::new(Status::Linked));
        let cancel = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        relay(
            core_a,
            peer_b,
            status.clone(),
            cancel.clone(),
            sockets.clone(),
        );
        peer_a.write_all(&[7, 9]).unwrap();
        let mut got = [0u8; 2];
        core_b.read_exact(&mut got).unwrap();
        assert_eq!(got, [7, 9]);
        core_b.write_all(&[1, 2]).unwrap();
        peer_a.read_exact(&mut got).unwrap();
        assert_eq!(got, [1, 2]);
        cancel.store(true, Ordering::SeqCst);
        for s in lock(&sockets).iter() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    fn pipe() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, client)
    }

    #[test]
    fn game_reads_a_gb_header() {
        let path = std::env::temp_dir().join(format!("magi_netplay_{}.gb", std::process::id()));
        let mut rom = vec![0u8; 0x150];
        rom[0x134..0x13F].copy_from_slice(b"TETRIS\0\0\0\0\0");
        std::fs::write(&path, &rom).unwrap();
        let game = Game::from_rom("gb", path.to_str().unwrap());
        assert_eq!(game.code.len(), 4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_host_and_a_joiner_find_each_other() {
        let game = Game {
            system: "gb".into(),
            title: "Tetris".into(),
            code: "TETR".into(),
        };
        let mut host = Session::host_local(Transport::Wifi, game.clone(), 0);
        wait_for(&host, |s| matches!(s, Status::Waiting), 3);

        let mut join = Session::join_local(Transport::Wifi, game, 0);
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut found = Vec::new();
        while Instant::now() < deadline {
            found = join.hosts();
            if !found.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !found.is_empty(),
            "the joiner did not see the host: {:?}",
            join.status()
        );
        join.connect(found[0].clone());
        wait_for(&join, |s| matches!(s, Status::Linked), 6);
        wait_for(&host, |s| matches!(s, Status::Linked), 6);
        join.stop();
        host.stop();
    }

    fn wait_for(session: &Session, done: impl Fn(&Status) -> bool, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let status = session.status();
            if done(&status) {
                return;
            }
            if let Status::Error(e) = &status {
                panic!("netplay error: {e}");
            }
            assert!(Instant::now() < deadline, "netplay state: {status:?}");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
