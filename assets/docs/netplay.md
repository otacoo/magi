# Netplay

Game Boy and Game Boy Color games can link over a local network. One console
hosts, the other joins, and the games exchange their link-cable bytes over TCP.
Each device runs its own copy with its own save.

Netplay is only offered for GB/GBC games running on gambatte, the default core
for both systems. GBA, and GB games running on another core, do not show the
menu entry.

## Start a link

Both players open the game and wait at its own link screen. On one device:

1. Press **MENU**, then choose **NETPLAY**.
2. Choose **HOST GAME** or **JOIN GAME**.
3. Choose **HOTSPOT** or **WIFI**.

The host shows a four-character code. The joiner scans for hosts and picks that
code. When both devices say **LINKED**, enter the game's link mode.

### Hotspot

The host creates a private access point named `MAGI-<code>`. The joiner joins
it automatically after picking the code. No router is needed. Both devices
leave their normal Wi-Fi for the session, and it is restored when the link
ends.

### Wi-Fi

Both devices must already be on the same network. The host advertises the game
and its code on that network; the joiner scans and picks it. This keeps the
normal Wi-Fi connection.

Netplay is local only. It does not work over the internet.

## What crosses the link

Only the serial bytes the games would put on a link cable. Saves, save states
and the rest of the frame stay on each device. A trade therefore uses both
players' own saves, and two different games can link when they speak the same
protocol.

The trade-off is latency: every link byte waits for the other device. Slow
networks make the link slower, but both games wait together, so they stay in
step.

## While linked

- **SAVE STATE**, **LOAD STATE**, **CORE OPTIONS**, rewind and fast-forward are
  off. A state load would put one game out of step with the other.
- Opening the pause menu pauses the game. The other device waits at its next
  link transfer, so keep menu visits short.
- **MENU**, then **NETPLAY**, then **END NETPLAY** ends the link. Closing the
  lid, powering off or losing the network also ends it.
- If the other player disappears, MAGI ends the link and shows
  `THE OTHER PLAYER LEFT`. The game may show its own link error.

## If it does not connect

| Message | What to try |
|---|---|
| `NO HOSTS FOUND` | Make sure both devices chose the same transport, then rescan. On Wi-Fi, check that both are on the same network. |
| `HOTSPOT: RADIO REFUSED AP MODE` | The Wi-Fi driver would not host an access point. Use **WIFI** instead. |
| `HOTSPOT: NO ANSWER` | Keep the devices close and host first, then join. |
| `WRONG CODE` | Pick the code shown on the host, not an older scan result. |
| The game shows a link error | Both games must be in their link mode and use compatible link protocols. |

A private hotspot is the most reliable choice when the normal network blocks
broadcast traffic or the two devices are on different networks.
