//! EGL/GLES presentation for the H700 Mali GPU.
//!
//! Software cores upload one frame per tick; hardware cores draw into an FBO
//! this module owns, and the same shader composites either source with the
//! bezel/CRT overlay and the warmth tint. The software presenter remains the
//! fallback.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use libloading::Library;

use crate::util;

const EGL_NONE: i32 = 0x3038;
const EGL_ALPHA_SIZE: i32 = 0x3021;
const EGL_BLUE_SIZE: i32 = 0x3022;
const EGL_GREEN_SIZE: i32 = 0x3023;
const EGL_RED_SIZE: i32 = 0x3024;
const EGL_SURFACE_TYPE: i32 = 0x3033;
const EGL_RENDERABLE_TYPE: i32 = 0x3040;
const EGL_CONTEXT_CLIENT_VERSION: i32 = 0x3098;
const EGL_OPENGL_ES_API: u32 = 0x30A0;
const EGL_WINDOW_BIT: i32 = 0x0004;
const EGL_OPENGL_ES2_BIT: i32 = 0x0004;

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE0: u32 = 0x84C0;
const GL_BGRA_EXT: u32 = 0x80E1;
const GL_RGBA: u32 = 0x1908;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_RENDERBUFFER: u32 = 0x8D41;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_DEPTH_ATTACHMENT: u32 = 0x8D00;
const GL_DEPTH_STENCIL_ATTACHMENT: u32 = 0x821A;
const GL_DEPTH24_STENCIL8: u32 = 0x88F0;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const GL_DEPTH_COMPONENT16: u32 = 0x81A5;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
const GL_PACK_ALIGNMENT: u32 = 0x0D05;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_NEAREST: u32 = 0x2600;
const GL_LINEAR: u32 = 0x2601;
const GL_CLAMP_TO_EDGE: u32 = 0x812F;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_STATIC_DRAW: u32 = 0x88E4;
const GL_FLOAT: u32 = 0x1406;
const GL_FALSE: u8 = 0;
const GL_TRIANGLE_STRIP: u32 = 0x0005;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_BLEND: u32 = 0x0BE2;
const GL_DEPTH_TEST: u32 = 0x0B71;
const GL_CULL_FACE: u32 = 0x0B44;
const GL_STENCIL_TEST: u32 = 0x0B90;
const GL_SCISSOR_TEST: u32 = 0x0C11;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_INFO_LOG_LENGTH: u32 = 0x8B84;

const VERT: &str = r#"
attribute vec2 a_pos;
uniform vec4 u_rect;
uniform vec2 u_target;
uniform vec4 u_uv;
varying vec2 v_uv;
void main() {
    v_uv = u_uv.xy + a_pos * u_uv.zw;
    vec2 p = (u_rect.xy + a_pos * u_rect.zw) / u_target;
    gl_Position = vec4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
}
"#;

const VERT_PLAIN: &str = r#"
attribute vec2 a_pos;
uniform vec4 u_rect;
uniform vec2 u_target;
uniform vec4 u_uv;
varying vec2 v_uv;
void main() {
    v_uv = u_uv.xy + a_pos * u_uv.zw;
    vec2 p = (u_rect.xy + a_pos * u_rect.zw) / u_target;
    gl_Position = vec4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
}
"#;

// Modes 1-4 are the public-domain handheld shaders from libretro/glsl-shaders
// (Color Mangler, lcd3x, dot); see THIRD_PARTY_NOTICES.md.
// Mode 5 is MAGI's own LCD-accurate approximation: GB dots are ink over a
// lit panel, gaps take their neighbour's ink, and the dots cast a shadow.
// Colour screens keep their sampled colour and get a square grid.
const FRAG: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_tex;
uniform vec3 u_gain;
uniform float u_alpha;
uniform float u_overlay;
uniform int u_mode;
uniform int u_sys;
uniform vec2 u_in;
uniform vec2 u_out;
varying vec2 v_uv;

float shade_at(vec2 at) {
    return dot(texture2D(u_tex, clamp(at, vec2(0.0), vec2(1.0))).rgb,
        vec3(1.0 / 3.0));
}

float gb_ink(float g) {
    return min(1.153846 * (1.0 - g) + 0.05, 1.0);
}

float gb_ink_at(vec2 at) {
    return gb_ink(shade_at(at));
}

void main() {
    vec2 uv = v_uv;
    vec4 c;
    if (u_mode == 1) {
        vec3 s = pow(texture2D(u_tex, uv).rgb, vec3(3.2, 3.2, 3.2));
        s = clamp(s * 0.94, 0.0, 1.0);
        s = vec3(
            dot(s, vec3(0.82, 0.24, -0.06)),
            dot(s, vec3(0.125, 0.665, 0.21)),
            dot(s, vec3(0.195, 0.075, 0.73))
        );
        c = vec4(pow(clamp(s, 0.0, 1.0), vec3(1.0 / 2.2, 1.0 / 2.2, 1.0 / 2.2)), 1.0);
    } else if (u_mode == 2) {
        vec3 s = pow(texture2D(u_tex, uv).rgb, vec3(1.2, 1.2, 1.2));
        s = clamp(s * 0.94, 0.0, 1.0);
        s = vec3(
            dot(s, vec3(0.82, 0.24, -0.06)),
            dot(s, vec3(0.125, 0.665, 0.21)),
            dot(s, vec3(0.195, 0.075, 0.73))
        );
        c = vec4(pow(clamp(s, 0.0, 1.0), vec3(1.0 / 2.2, 1.0 / 2.2, 1.0 / 2.2)), 1.0);
    } else if (u_mode == 3) {
        const vec3 offsets = vec3(3.141592654, 3.141592654, 3.141592654)
            * vec3(0.5, 0.5 - 2.0 / 3.0, 0.5 - 4.0 / 3.0);
        vec2 omega = vec2(6.283185307, 6.283185307) * u_in;
        vec2 angle = uv * omega;
        float yfactor = (16.0 + sin(angle.y)) / 17.0;
        vec3 xfactors = (4.0 + sin(angle.x + offsets)) / 5.0;
        c = vec4(texture2D(u_tex, uv).rgb * yfactor * xfactors, 1.0);
    } else if (u_mode == 4) {
        vec2 pix = uv * u_in;
        vec3 s = texture2D(u_tex, uv).rgb;
        float bright = dot(s, vec3(0.30, 0.59, 0.11));
        float bloom = mix(1.05, 0.95, bright);
        vec3 mid = s * exp(-2.4 * length(fract(pix) - vec2(0.5, 0.5)) * bloom);
        vec3 sum = mid
            + s * exp(-2.4 * length(fract(pix) - vec2(-0.5, -0.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(0.5, -0.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(1.5, -0.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(-0.5, 0.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(1.5, 0.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(-0.5, 1.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(0.5, 1.5)) * bloom)
            + s * exp(-2.4 * length(fract(pix) - vec2(1.5, 1.5)) * bloom);
        c = vec4(mix(1.2 * mid, sum, 0.65), 1.0);
    } else if (u_mode == 5) {
        vec2 q = uv * u_in;
        vec2 texel = 1.0 / u_in;
        vec2 scale = u_out / max(u_in, vec2(1.0));
        vec2 gap_px = clamp(vec2(1.0) / max(scale, vec2(1.0)), 0.0, 0.5);
        vec2 on_gap = step(fract(q), gap_px);
        vec3 rgb;
        if (u_sys == 1) {
            // A Game Boy dot is a share of ink over the lit panel. A gap
            // takes ink from its left or top neighbour; where gaps cross,
            // only the panel shows. The dots cast down and to the left.
            float a = gb_ink(shade_at(uv));
            float left = q.x > 1.0 ? gb_ink_at(uv - vec2(texel.x, 0.0)) : 0.0;
            float up = q.y > 1.0 ? gb_ink_at(uv - vec2(0.0, texel.y)) : 0.0;
            float gap_x = 0.38 * (a + left);
            float gap_y = 0.38 * (a + up);
            float dot_a = on_gap.x > 0.5
                ? (on_gap.y > 0.5 ? 0.0 : gap_x)
                : (on_gap.y > 0.5 ? gap_y : a);
            vec2 from = uv + vec2(0.5, -0.5) * texel;
            float sh = gb_ink_at(from - 0.5 * texel)
                + gb_ink_at(from + 0.5 * texel)
                + gb_ink_at(from + vec2(0.5, -0.5) * texel)
                + gb_ink_at(from + vec2(-0.5, 0.5) * texel);
            sh *= 0.25 * 0.30 * (1.0 - 0.75 * dot_a);
            vec3 ink = vec3(0.0, 0.353, 0.667);
            vec3 panel = vec3(0.631, 0.631, 0.0);
            rgb = ink * dot_a * (1.0 - 0.7 * dot_a * dot_a)
                + panel * (1.0 - sh) * (0.80 - 0.63 * dot_a);
        } else {
            rgb = texture2D(u_tex, (floor(q) + vec2(0.5)) / u_in).rgb;
            float grid = max(on_gap.x, on_gap.y)
                * smoothstep(1.2, 2.0, min(scale.x, scale.y));
            rgb = mix(rgb, vec3(0.02, 0.05, 0.035), grid * 0.75);
        }
        c = vec4(rgb, 1.0);
    } else {
        c = texture2D(u_tex, uv);
    }
    gl_FragColor = vec4(c.rgb * u_gain, mix(1.0, c.a, u_overlay) * u_alpha);
}
"#;

// Fallback for drivers that reject the shader branches: the original
// composite shader, with the shader uniforms simply absent (-1 locations).
const FRAG_PLAIN: &str = r#"
precision mediump float;
uniform sampler2D u_tex;
uniform vec3 u_gain;
uniform float u_alpha;
uniform float u_overlay;
varying vec2 v_uv;
void main() {
    vec4 c = texture2D(u_tex, v_uv);
    gl_FragColor = vec4(c.rgb * u_gain, mix(1.0, c.a, u_overlay) * u_alpha);
}
"#;

type Ptr = *mut c_void;

struct Egl {
    get_display: unsafe extern "C" fn(Ptr) -> Ptr,
    initialize: unsafe extern "C" fn(Ptr, *mut i32, *mut i32) -> u32,
    bind_api: unsafe extern "C" fn(u32) -> u32,
    choose_config: unsafe extern "C" fn(Ptr, *const i32, *mut Ptr, i32, *mut i32) -> u32,
    create_window_surface: unsafe extern "C" fn(Ptr, Ptr, Ptr, *const i32) -> Ptr,
    create_context: unsafe extern "C" fn(Ptr, Ptr, Ptr, *const i32) -> Ptr,
    make_current: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr) -> u32,
    swap_buffers: unsafe extern "C" fn(Ptr, Ptr) -> u32,
    swap_interval: unsafe extern "C" fn(Ptr, i32) -> u32,
    destroy_surface: unsafe extern "C" fn(Ptr, Ptr) -> u32,
    destroy_context: unsafe extern "C" fn(Ptr, Ptr) -> u32,
    terminate: unsafe extern "C" fn(Ptr) -> u32,
    get_error: unsafe extern "C" fn() -> i32,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *const c_void,
    _lib: Library,
}

unsafe fn load<T: Copy>(lib: &Library, egl: Option<&Egl>, name: &str) -> Result<T, String> {
    if let Ok(sym) = lib.get::<T>(name.as_bytes()) {
        return Ok(*sym);
    }
    if let Some(egl) = egl {
        let Ok(cname) = CString::new(name) else {
            return Err(format!("{name}: invalid name"));
        };
        let p = (egl.get_proc_address)(cname.as_ptr());
        if !p.is_null() {
            return Ok(std::mem::transmute_copy(&p));
        }
    }
    Err(format!("missing GL symbol {name}"))
}

impl Egl {
    fn load() -> Result<Self, String> {
        let lib = unsafe { Library::new("libEGL.so.1") }
            .or_else(|_| unsafe { Library::new("libEGL.so") })
            .map_err(|e| format!("libEGL: {e}"))?;
        unsafe {
            Ok(Self {
                get_display: load::<unsafe extern "C" fn(Ptr) -> Ptr>(&lib, None, "eglGetDisplay")?,
                initialize: load::<unsafe extern "C" fn(Ptr, *mut i32, *mut i32) -> u32>(
                    &lib,
                    None,
                    "eglInitialize",
                )?,
                bind_api: load(&lib, None, "eglBindAPI")?,
                choose_config: load(&lib, None, "eglChooseConfig")?,
                create_window_surface: load(&lib, None, "eglCreateWindowSurface")?,
                create_context: load(&lib, None, "eglCreateContext")?,
                make_current: load(&lib, None, "eglMakeCurrent")?,
                swap_buffers: load(&lib, None, "eglSwapBuffers")?,
                swap_interval: load(&lib, None, "eglSwapInterval")?,
                destroy_surface: load(&lib, None, "eglDestroySurface")?,
                destroy_context: load(&lib, None, "eglDestroyContext")?,
                terminate: load(&lib, None, "eglTerminate")?,
                get_error: load(&lib, None, "eglGetError")?,
                get_proc_address: load(&lib, None, "eglGetProcAddress")?,
                _lib: lib,
            })
        }
    }

    fn fail(&self, what: &str) -> String {
        let code = unsafe { (self.get_error)() };
        format!("{what}: EGL 0x{code:x}")
    }
}

struct Gles {
    clear: unsafe extern "C" fn(u32),
    clear_color: unsafe extern "C" fn(f32, f32, f32, f32),
    viewport: unsafe extern "C" fn(i32, i32, i32, i32),
    gen_textures: unsafe extern "C" fn(i32, *mut u32),
    delete_textures: unsafe extern "C" fn(i32, *const u32),
    bind_texture: unsafe extern "C" fn(u32, u32),
    tex_parameteri: unsafe extern "C" fn(u32, u32, i32),
    tex_image_2d: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void),
    tex_sub_image_2d: unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void),
    read_pixels: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void),
    pixel_storei: unsafe extern "C" fn(u32, i32),
    active_texture: unsafe extern "C" fn(u32),
    gen_buffers: unsafe extern "C" fn(i32, *mut u32),
    bind_buffer: unsafe extern "C" fn(u32, u32),
    buffer_data: unsafe extern "C" fn(u32, isize, *const c_void, u32),
    delete_buffers: unsafe extern "C" fn(i32, *const u32),
    enable_vertex_attrib_array: unsafe extern "C" fn(u32),
    disable_vertex_attrib_array: unsafe extern "C" fn(u32),
    vertex_attrib_pointer: unsafe extern "C" fn(u32, i32, u32, u8, i32, *const c_void),
    create_shader: unsafe extern "C" fn(u32) -> u32,
    shader_source: unsafe extern "C" fn(u32, i32, *const *const c_char, *const i32),
    compile_shader: unsafe extern "C" fn(u32),
    get_shaderiv: unsafe extern "C" fn(u32, u32, *mut i32),
    get_shader_info_log: unsafe extern "C" fn(u32, i32, *mut i32, *mut c_char),
    delete_shader: unsafe extern "C" fn(u32),
    create_program: unsafe extern "C" fn() -> u32,
    attach_shader: unsafe extern "C" fn(u32, u32),
    bind_attrib_location: unsafe extern "C" fn(u32, u32, *const c_char),
    link_program: unsafe extern "C" fn(u32),
    get_programiv: unsafe extern "C" fn(u32, u32, *mut i32),
    get_program_info_log: unsafe extern "C" fn(u32, i32, *mut i32, *mut c_char),
    delete_program: unsafe extern "C" fn(u32),
    use_program: unsafe extern "C" fn(u32),
    get_uniform_location: unsafe extern "C" fn(u32, *const c_char) -> i32,
    uniform_1i: unsafe extern "C" fn(i32, i32),
    uniform_1f: unsafe extern "C" fn(i32, f32),
    uniform_2f: unsafe extern "C" fn(i32, f32, f32),
    uniform_3f: unsafe extern "C" fn(i32, f32, f32, f32),
    uniform_4f: unsafe extern "C" fn(i32, f32, f32, f32, f32),
    draw_arrays: unsafe extern "C" fn(u32, i32, i32),
    enable: unsafe extern "C" fn(u32),
    disable: unsafe extern "C" fn(u32),
    blend_func: unsafe extern "C" fn(u32, u32),
    color_mask: unsafe extern "C" fn(u8, u8, u8, u8),
    gen_framebuffers: unsafe extern "C" fn(i32, *mut u32),
    delete_framebuffers: unsafe extern "C" fn(i32, *const u32),
    bind_framebuffer: unsafe extern "C" fn(u32, u32),
    framebuffer_texture_2d: unsafe extern "C" fn(u32, u32, u32, u32, i32),
    gen_renderbuffers: unsafe extern "C" fn(i32, *mut u32),
    delete_renderbuffers: unsafe extern "C" fn(i32, *const u32),
    bind_renderbuffer: unsafe extern "C" fn(u32, u32),
    renderbuffer_storage: unsafe extern "C" fn(u32, u32, i32, i32),
    framebuffer_renderbuffer: unsafe extern "C" fn(u32, u32, u32, u32),
    check_framebuffer_status: unsafe extern "C" fn(u32) -> u32,
    _lib: Library,
}

impl Gles {
    fn load(egl: &Egl) -> Result<Self, String> {
        let lib = unsafe { Library::new("libGLESv2.so.2") }
            .or_else(|_| unsafe { Library::new("libGLESv2.so") })
            .map_err(|e| format!("libGLESv2: {e}"))?;
        unsafe {
            Ok(Self {
                clear: load(&lib, Some(egl), "glClear")?,
                clear_color: load(&lib, Some(egl), "glClearColor")?,
                viewport: load(&lib, Some(egl), "glViewport")?,
                gen_textures: load(&lib, Some(egl), "glGenTextures")?,
                delete_textures: load(&lib, Some(egl), "glDeleteTextures")?,
                bind_texture: load(&lib, Some(egl), "glBindTexture")?,
                tex_parameteri: load(&lib, Some(egl), "glTexParameteri")?,
                tex_image_2d: load(&lib, Some(egl), "glTexImage2D")?,
                tex_sub_image_2d: load(&lib, Some(egl), "glTexSubImage2D")?,
                read_pixels: load(&lib, Some(egl), "glReadPixels")?,
                pixel_storei: load(&lib, Some(egl), "glPixelStorei")?,
                active_texture: load(&lib, Some(egl), "glActiveTexture")?,
                gen_buffers: load(&lib, Some(egl), "glGenBuffers")?,
                bind_buffer: load(&lib, Some(egl), "glBindBuffer")?,
                buffer_data: load(&lib, Some(egl), "glBufferData")?,
                delete_buffers: load(&lib, Some(egl), "glDeleteBuffers")?,
                enable_vertex_attrib_array: load(&lib, Some(egl), "glEnableVertexAttribArray")?,
                disable_vertex_attrib_array: load(&lib, Some(egl), "glDisableVertexAttribArray")?,
                vertex_attrib_pointer: load(&lib, Some(egl), "glVertexAttribPointer")?,
                create_shader: load(&lib, Some(egl), "glCreateShader")?,
                shader_source: load(&lib, Some(egl), "glShaderSource")?,
                compile_shader: load(&lib, Some(egl), "glCompileShader")?,
                get_shaderiv: load(&lib, Some(egl), "glGetShaderiv")?,
                get_shader_info_log: load(&lib, Some(egl), "glGetShaderInfoLog")?,
                delete_shader: load(&lib, Some(egl), "glDeleteShader")?,
                create_program: load(&lib, Some(egl), "glCreateProgram")?,
                attach_shader: load(&lib, Some(egl), "glAttachShader")?,
                bind_attrib_location: load(&lib, Some(egl), "glBindAttribLocation")?,
                link_program: load(&lib, Some(egl), "glLinkProgram")?,
                get_programiv: load(&lib, Some(egl), "glGetProgramiv")?,
                get_program_info_log: load(&lib, Some(egl), "glGetProgramInfoLog")?,
                delete_program: load(&lib, Some(egl), "glDeleteProgram")?,
                use_program: load(&lib, Some(egl), "glUseProgram")?,
                get_uniform_location: load(&lib, Some(egl), "glGetUniformLocation")?,
                uniform_1i: load(&lib, Some(egl), "glUniform1i")?,
                uniform_1f: load(&lib, Some(egl), "glUniform1f")?,
                uniform_2f: load(&lib, Some(egl), "glUniform2f")?,
                uniform_3f: load(&lib, Some(egl), "glUniform3f")?,
                uniform_4f: load(&lib, Some(egl), "glUniform4f")?,
                draw_arrays: load(&lib, Some(egl), "glDrawArrays")?,
                enable: load(&lib, Some(egl), "glEnable")?,
                disable: load(&lib, Some(egl), "glDisable")?,
                blend_func: load(&lib, Some(egl), "glBlendFunc")?,
                color_mask: load(&lib, Some(egl), "glColorMask")?,
                gen_framebuffers: load(&lib, Some(egl), "glGenFramebuffers")?,
                delete_framebuffers: load(&lib, Some(egl), "glDeleteFramebuffers")?,
                bind_framebuffer: load(&lib, Some(egl), "glBindFramebuffer")?,
                framebuffer_texture_2d: load(&lib, Some(egl), "glFramebufferTexture2D")?,
                gen_renderbuffers: load(&lib, Some(egl), "glGenRenderbuffers")?,
                delete_renderbuffers: load(&lib, Some(egl), "glDeleteRenderbuffers")?,
                bind_renderbuffer: load(&lib, Some(egl), "glBindRenderbuffer")?,
                renderbuffer_storage: load(&lib, Some(egl), "glRenderbufferStorage")?,
                framebuffer_renderbuffer: load(&lib, Some(egl), "glFramebufferRenderbuffer")?,
                check_framebuffer_status: load(&lib, Some(egl), "glCheckFramebufferStatus")?,
                _lib: lib,
            })
        }
    }
}

#[repr(C)]
struct Window {
    width: u16,
    height: u16,
}

/// A UI layer the GPU composites over the game frame: the canvas pixels and
/// the rectangle that changed since the last upload.
pub struct Layer<'a> {
    pub px: &'a [u32],
    pub w: u32,
    pub h: u32,
    pub rect: (i32, i32, u32, u32),
}

/// The framebuffer a hardware-rendered core draws into. The colour texture is
/// composited by [`Gpu::present_hw`].
struct HwFbo {
    fbo: u32,
    tex: u32,
    depth: u32,
    w: i32,
    h: i32,
}

pub struct Gpu {
    egl: Egl,
    gl: Gles,
    display: Ptr,
    surface: Ptr,
    context: Ptr,
    _window: Box<Window>,
    w: i32,
    h: i32,
    program: u32,
    u_tex: i32,
    u_rect: i32,
    u_target: i32,
    u_uv: i32,
    u_gain: i32,
    u_alpha: i32,
    u_overlay: i32,
    u_mode: i32,
    u_sys: i32,
    u_in: i32,
    u_out: i32,
    vbo: u32,
    game_tex: u32,
    game_size: (i32, i32),
    shader: u8,
    shader_ok: bool,
    sys: i32,
    overlay_tex: u32,
    overlay_size: (i32, i32),
    has_overlay: bool,
    ui_tex: u32,
    ui_size: (i32, i32),
    ui_rect: Option<(i32, i32, u32, u32)>,
    ui_buf: Vec<u8>,
    frame_buf: Vec<u8>,
    hw: Option<HwFbo>,
}

impl Gpu {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let egl = Egl::load()?;
        let gl = Gles::load(&egl)?;
        let display;
        let surface;
        let context;
        let mut window = Box::new(Window {
            width: width as u16,
            height: height as u16,
        });
        unsafe {
            display = (egl.get_display)(ptr::null_mut());
            if display.is_null() {
                return Err(egl.fail("eglGetDisplay"));
            }
            if (egl.initialize)(display, ptr::null_mut(), ptr::null_mut()) == 0 {
                return Err(egl.fail("eglInitialize"));
            }
            if (egl.bind_api)(EGL_OPENGL_ES_API) == 0 {
                return Err(egl.fail("eglBindAPI"));
            }
            let attrs = [
                EGL_SURFACE_TYPE,
                EGL_WINDOW_BIT,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                0,
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_ES2_BIT,
                EGL_NONE,
            ];
            let mut config: Ptr = ptr::null_mut();
            let mut found = 0;
            if (egl.choose_config)(display, attrs.as_ptr(), &mut config, 1, &mut found) == 0
                || found == 0
            {
                return Err(egl.fail("eglChooseConfig"));
            }
            let native = &mut *window as *mut Window as Ptr;
            surface = (egl.create_window_surface)(display, config, native, ptr::null());
            if surface.is_null() {
                return Err(egl.fail("eglCreateWindowSurface"));
            }
            let context_attrs = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            context =
                (egl.create_context)(display, config, ptr::null_mut(), context_attrs.as_ptr());
            if context.is_null() {
                let _ = (egl.destroy_surface)(display, surface);
                return Err(egl.fail("eglCreateContext"));
            }
            if (egl.make_current)(display, surface, surface, context) == 0 {
                let _ = (egl.destroy_context)(display, context);
                let _ = (egl.destroy_surface)(display, surface);
                return Err(egl.fail("eglMakeCurrent"));
            }
            (egl.swap_interval)(display, 1);
        }

        let (program, shader_ok) = match unsafe { make_program(&gl, VERT, FRAG) } {
            Ok(program) => (program, true),
            Err(e) => {
                util::log(&format!("{e}; using the plain shader"));
                (unsafe { make_program(&gl, VERT_PLAIN, FRAG_PLAIN) }?, false)
            }
        };
        let mut gpu = Self {
            egl,
            gl,
            display,
            surface,
            context,
            _window: window,
            w: width as i32,
            h: height as i32,
            program,
            u_tex: 0,
            u_rect: 0,
            u_target: 0,
            u_uv: 0,
            u_gain: 0,
            u_alpha: 0,
            u_overlay: 0,
            u_mode: 0,
            u_sys: 0,
            u_in: 0,
            u_out: 0,
            vbo: 0,
            game_tex: 0,
            game_size: (0, 0),
            shader: 0,
            shader_ok,
            sys: 0,
            overlay_tex: 0,
            overlay_size: (0, 0),
            has_overlay: false,
            ui_tex: 0,
            ui_size: (0, 0),
            ui_rect: None,
            ui_buf: Vec::new(),
            frame_buf: Vec::new(),
            hw: None,
        };
        unsafe {
            gpu.u_tex = uniform(&gpu.gl, program, "u_tex");
            gpu.u_rect = uniform(&gpu.gl, program, "u_rect");
            gpu.u_target = uniform(&gpu.gl, program, "u_target");
            gpu.u_uv = uniform(&gpu.gl, program, "u_uv");
            gpu.u_gain = uniform(&gpu.gl, program, "u_gain");
            gpu.u_alpha = uniform(&gpu.gl, program, "u_alpha");
            gpu.u_overlay = uniform(&gpu.gl, program, "u_overlay");
            gpu.u_mode = uniform(&gpu.gl, program, "u_mode");
            gpu.u_sys = uniform(&gpu.gl, program, "u_sys");
            gpu.u_in = uniform(&gpu.gl, program, "u_in");
            gpu.u_out = uniform(&gpu.gl, program, "u_out");
            (gpu.gl.gen_buffers)(1, &mut gpu.vbo);
            (gpu.gl.bind_buffer)(GL_ARRAY_BUFFER, gpu.vbo);
            let verts: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            (gpu.gl.buffer_data)(
                GL_ARRAY_BUFFER,
                std::mem::size_of_val(&verts) as isize,
                verts.as_ptr() as *const c_void,
                GL_STATIC_DRAW,
            );
            (gpu.gl.enable_vertex_attrib_array)(0);
            (gpu.gl.vertex_attrib_pointer)(0, 2, GL_FLOAT, GL_FALSE, 0, ptr::null());
            (gpu.gl.viewport)(0, 0, gpu.w, gpu.h);
            (gpu.gl.clear_color)(0.0, 0.0, 0.0, 1.0);
            (gpu.gl.use_program)(gpu.program);
            (gpu.gl.uniform_1i)(gpu.u_tex, 0);
            (gpu.gl.uniform_1i)(gpu.u_mode, 0);
            (gpu.gl.uniform_1i)(gpu.u_sys, 0);
            (gpu.gl.uniform_2f)(gpu.u_in, 1.0, 1.0);
            (gpu.gl.uniform_2f)(gpu.u_out, 1.0, 1.0);
        }
        Ok(gpu)
    }

    /// Tell the LCD-accurate shader which panel to imitate.
    pub fn set_system(&mut self, system: &str) {
        let code = match system {
            "gb" => 1,
            "gbc" => 2,
            "gba" => 3,
            _ => 0,
        };
        self.sys = code;
        unsafe { (self.gl.uniform_1i)(self.u_sys, code) };
    }

    /// Select the game-image shader: 0 = nearest, 1-4 = colour and LCD
    /// effects, 5 = LCD accurate.
    pub fn set_shader(&mut self, mode: u8) {
        let mode = if self.shader_ok { mode } else { 0 };
        if self.shader == mode {
            return;
        }
        self.shader = mode;
        let filter = GL_NEAREST as i32;
        unsafe {
            if self.game_tex != 0 {
                (self.gl.bind_texture)(GL_TEXTURE_2D, self.game_tex);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            }
            if let Some(hw) = self.hw.as_ref() {
                (self.gl.bind_texture)(GL_TEXTURE_2D, hw.tex);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            }
        }
    }

    pub fn set_overlay(&mut self, px: Option<&[u32]>, w: u32, h: u32) {
        let Some(px) = px else {
            self.has_overlay = false;
            return;
        };
        if px.len() < (w * h) as usize {
            self.has_overlay = false;
            return;
        }
        let (w, h) = (w as i32, h as i32);
        if self.overlay_tex == 0 || self.overlay_size != (w, h) {
            if self.overlay_tex != 0 {
                unsafe { (self.gl.delete_textures)(1, &self.overlay_tex) };
            }
            unsafe {
                (self.gl.gen_textures)(1, &mut self.overlay_tex);
                (self.gl.bind_texture)(GL_TEXTURE_2D, self.overlay_tex);
                (self.gl.tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_BGRA_EXT as i32,
                    w,
                    h,
                    0,
                    GL_BGRA_EXT,
                    GL_UNSIGNED_BYTE,
                    ptr::null(),
                );
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
            }
            self.overlay_size = (w, h);
        }
        unsafe {
            (self.gl.bind_texture)(GL_TEXTURE_2D, self.overlay_tex);
            (self.gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                0,
                0,
                w,
                h,
                GL_BGRA_EXT,
                GL_UNSIGNED_BYTE,
                px.as_ptr() as *const c_void,
            );
        }
        self.has_overlay = true;
    }

    /// Upload the UI layer (the overlays and the menu) so the GPU can
    /// composite it over the game frame: only the rectangle that changed is
    /// sent, the rest of the texture keeps the previous frame.
    pub fn set_ui(&mut self, layer: Option<&Layer>) -> bool {
        let Some(layer) = layer else {
            self.ui_rect = None;
            return true;
        };
        let (x, y, w, h) = layer.rect;
        if w == 0 || h == 0 || x < 0 || y < 0 {
            self.ui_rect = None;
            return true;
        }
        let (tw, th) = (layer.w as i32, layer.h as i32);
        if x + w as i32 > tw || y + h as i32 > th {
            self.ui_rect = None;
            return false;
        }
        if self.ui_tex == 0 || self.ui_size != (tw, th) {
            if self.ui_tex != 0 {
                unsafe { (self.gl.delete_textures)(1, &self.ui_tex) };
            }
            unsafe {
                (self.gl.gen_textures)(1, &mut self.ui_tex);
                (self.gl.bind_texture)(GL_TEXTURE_2D, self.ui_tex);
                (self.gl.tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_BGRA_EXT as i32,
                    tw,
                    th,
                    0,
                    GL_BGRA_EXT,
                    GL_UNSIGNED_BYTE,
                    ptr::null(),
                );
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
            }
            self.ui_size = (tw, th);
        }
        // The canvas rows are contiguous, but the upload wants a rectangle.
        self.ui_buf.clear();
        self.ui_buf.reserve((w as usize) * (h as usize) * 4);
        for row in 0..h as usize {
            let start = (y as usize + row) * layer.w as usize + x as usize;
            let Some(slice) = layer.px.get(start..start + w as usize) else {
                self.ui_rect = None;
                return false;
            };
            for &v in slice {
                // The canvas is 0xAARRGGBB: in memory that is B,G,R,A, which
                // is what the BGRA texture wants.
                self.ui_buf.push(v as u8);
                self.ui_buf.push((v >> 8) as u8);
                self.ui_buf.push((v >> 16) as u8);
                self.ui_buf.push((v >> 24) as u8);
            }
        }
        unsafe {
            (self.gl.bind_texture)(GL_TEXTURE_2D, self.ui_tex);
            (self.gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                x,
                y,
                w as i32,
                h as i32,
                GL_BGRA_EXT,
                GL_UNSIGNED_BYTE,
                self.ui_buf.as_ptr() as *const c_void,
            );
        }
        self.ui_rect = Some((x, y, w, h));
        true
    }

    /// Create (or recreate) the FBO a hardware-rendered core draws into.
    /// Returns the framebuffer name the core must bind.
    pub fn hw_framebuffer(
        &mut self,
        width: u32,
        height: u32,
        stencil: bool,
    ) -> Result<u32, String> {
        let (w, h) = (width.max(1) as i32, height.max(1) as i32);
        if let Some(old) = self.hw.take() {
            unsafe { self.free_hw(&old) };
        }
        let mut want_stencil = stencil;
        loop {
            let result = self.try_hw_framebuffer(w, h, want_stencil);
            match result {
                Ok((fbo, tex, depth)) => {
                    self.hw = Some(HwFbo {
                        fbo,
                        tex,
                        depth,
                        w,
                        h,
                    });
                    return Ok(fbo);
                }
                Err(status) if want_stencil => {
                    util::log(&format!(
                        "hw: depth-stencil FBO incomplete (0x{status:x}); retrying depth only"
                    ));
                    want_stencil = false;
                }
                Err(status) => return Err(format!("FBO incomplete: 0x{status:x}")),
            }
        }
    }

    fn try_hw_framebuffer(&self, w: i32, h: i32, stencil: bool) -> Result<(u32, u32, u32), u32> {
        let filter = GL_NEAREST as i32;
        let (mut fbo, mut tex, mut depth) = (0, 0, 0);
        unsafe {
            (self.gl.gen_textures)(1, &mut tex);
            (self.gl.bind_texture)(GL_TEXTURE_2D, tex);
            (self.gl.tex_image_2d)(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as i32,
                w,
                h,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                ptr::null(),
            );
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
            (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
            (self.gl.gen_framebuffers)(1, &mut fbo);
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, fbo);
            (self.gl.framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                tex,
                0,
            );
            (self.gl.gen_renderbuffers)(1, &mut depth);
            (self.gl.bind_renderbuffer)(GL_RENDERBUFFER, depth);
            let (format, attachment) = if stencil {
                (GL_DEPTH24_STENCIL8, GL_DEPTH_STENCIL_ATTACHMENT)
            } else {
                (GL_DEPTH_COMPONENT16, GL_DEPTH_ATTACHMENT)
            };
            (self.gl.renderbuffer_storage)(GL_RENDERBUFFER, format, w, h);
            (self.gl.framebuffer_renderbuffer)(GL_FRAMEBUFFER, attachment, GL_RENDERBUFFER, depth);
            let status = (self.gl.check_framebuffer_status)(GL_FRAMEBUFFER);
            if status != GL_FRAMEBUFFER_COMPLETE {
                (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
                (self.gl.delete_framebuffers)(1, &fbo);
                (self.gl.delete_renderbuffers)(1, &depth);
                (self.gl.delete_textures)(1, &tex);
                return Err(status);
            }
            // A freshly allocated texture has undefined pixels; clear it so
            // the boot frames show black rather than garbage.
            (self.gl.color_mask)(1, 1, 1, 1);
            (self.gl.viewport)(0, 0, w, h);
            (self.gl.clear_color)(0.0, 0.0, 0.0, 1.0);
            (self.gl.clear)(GL_COLOR_BUFFER_BIT);
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
        }
        Ok((fbo, tex, depth))
    }

    /// Resolve a GL symbol for a hardware-rendered core. The symbol may live
    /// in the EGL dispatch table or in the loaded GLES library.
    pub fn proc_address(&self, name: &CStr) -> *mut c_void {
        unsafe {
            let p = (self.egl.get_proc_address)(name.as_ptr());
            if !p.is_null() {
                return p as *mut c_void;
            }
            if let Ok(sym) = self.gl._lib.get::<unsafe extern "C" fn()>(name.to_bytes()) {
                return *sym as usize as *mut c_void;
            }
        }
        ptr::null_mut()
    }

    /// Read the hardware framebuffer back into top-down `0xAARRGGBB`
    /// pixels. `flip_v` matches the presenter's bottom-left handling.
    pub fn read_hw(&mut self, flip_v: bool) -> Option<(Vec<u32>, u32, u32)> {
        let hw = self.hw.as_ref()?;
        let (w, h) = (hw.w, hw.h);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        unsafe {
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, hw.fbo);
            (self.gl.pixel_storei)(GL_PACK_ALIGNMENT, 1);
            (self.gl.read_pixels)(
                0,
                0,
                w,
                h,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                rgba.as_mut_ptr() as *mut c_void,
            );
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
        }
        let (wu, hu) = (w as usize, h as usize);
        let mut px = vec![0u32; wu * hu];
        for y in 0..hu {
            let sy = if flip_v { hu - 1 - y } else { y };
            for x in 0..wu {
                let i = (sy * wu + x) * 4;
                px[y * wu + x] = 0xFF00_0000
                    | ((rgba[i] as u32) << 16)
                    | ((rgba[i + 1] as u32) << 8)
                    | rgba[i + 2] as u32;
            }
        }
        Some((px, w as u32, h as u32))
    }

    unsafe fn free_hw(&self, hw: &HwFbo) {
        (self.gl.delete_framebuffers)(1, &hw.fbo);
        (self.gl.delete_renderbuffers)(1, &hw.depth);
        (self.gl.delete_textures)(1, &hw.tex);
    }

    pub fn present(
        &mut self,
        frame: *const u8,
        pitch: usize,
        fw: u32,
        fh: u32,
        fmt: u32,
        rect: (i32, i32, u32, u32),
        warmth: u8,
    ) -> bool {
        if frame.is_null() || fw == 0 || fh == 0 {
            return false;
        }
        let filter = GL_NEAREST as i32;
        let (iw, ih) = (fw as i32, fh as i32);
        let data = if fmt == 1 && pitch == iw as usize * 4 {
            frame as *const c_void
        } else {
            self.frame_buf.clear();
            match fmt {
                1 => {
                    self.frame_buf.reserve(iw as usize * ih as usize * 4);
                    for y in 0..ih as usize {
                        let row = unsafe { frame.add(y * pitch) };
                        self.frame_buf.extend_from_slice(unsafe {
                            std::slice::from_raw_parts(row, iw as usize * 4)
                        });
                    }
                }
                2 => {
                    self.frame_buf.reserve(iw as usize * ih as usize * 4);
                    for y in 0..ih as usize {
                        let row = unsafe { frame.add(y * pitch) as *const u16 };
                        for x in 0..iw as usize {
                            let p = unsafe { ptr::read_unaligned(row.add(x)) };
                            let r = ((p >> 11) & 0x1f) as u8;
                            let g = ((p >> 5) & 0x3f) as u8;
                            let b = (p & 0x1f) as u8;
                            self.frame_buf.push((b << 3) | (b >> 2));
                            self.frame_buf.push((g << 2) | (g >> 4));
                            self.frame_buf.push((r << 3) | (r >> 2));
                            self.frame_buf.push(255);
                        }
                    }
                }
                _ => {
                    self.frame_buf.reserve(iw as usize * ih as usize * 4);
                    for y in 0..ih as usize {
                        let row = unsafe { frame.add(y * pitch) as *const u16 };
                        for x in 0..iw as usize {
                            let p = unsafe { ptr::read_unaligned(row.add(x)) };
                            let r = (p >> 10) & 0x1f;
                            let g = (p >> 5) & 0x1f;
                            let b = p & 0x1f;
                            self.frame_buf.push(((b << 3) | (b >> 2)) as u8);
                            self.frame_buf.push(((g << 3) | (g >> 2)) as u8);
                            self.frame_buf.push(((r << 3) | (r >> 2)) as u8);
                            self.frame_buf.push(255);
                        }
                    }
                }
            }
            self.frame_buf.as_ptr() as *const c_void
        };
        if self.game_tex == 0 || self.game_size != (iw, ih) {
            if self.game_tex != 0 {
                unsafe { (self.gl.delete_textures)(1, &self.game_tex) };
            }
            unsafe {
                (self.gl.gen_textures)(1, &mut self.game_tex);
                (self.gl.bind_texture)(GL_TEXTURE_2D, self.game_tex);
                (self.gl.tex_image_2d)(
                    GL_TEXTURE_2D,
                    0,
                    GL_BGRA_EXT as i32,
                    iw,
                    ih,
                    0,
                    GL_BGRA_EXT,
                    GL_UNSIGNED_BYTE,
                    ptr::null(),
                );
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE as i32);
                (self.gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE as i32);
            }
            self.game_size = (iw, ih);
        }
        unsafe {
            (self.gl.bind_texture)(GL_TEXTURE_2D, self.game_tex);
            (self.gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
            (self.gl.tex_sub_image_2d)(
                GL_TEXTURE_2D,
                0,
                0,
                0,
                iw,
                ih,
                GL_BGRA_EXT,
                GL_UNSIGNED_BYTE,
                data,
            );
        }

        self.draw_scene(
            self.game_tex,
            (0.0, 0.0, 1.0, 1.0),
            rect,
            warmth,
            self.shader as i32,
            (iw as f32, ih as f32),
        );
        self.swap()
    }

    /// Composite the framebuffer a hardware-rendered core drew into. Cores
    /// asking for a bottom-left origin need the texture flipped vertically.
    pub fn present_hw(&mut self, rect: (i32, i32, u32, u32), warmth: u8, flip_v: bool) -> bool {
        let Some(hw) = self.hw.as_ref() else {
            return false;
        };
        let tex = hw.tex;
        let (iw, ih) = (hw.w as f32, hw.h as f32);
        let uv = if flip_v {
            (0.0, 1.0, 1.0, -1.0)
        } else {
            (0.0, 0.0, 1.0, 1.0)
        };
        self.draw_scene(tex, uv, rect, warmth, self.shader as i32, (iw, ih));
        self.swap()
    }

    fn swap(&mut self) -> bool {
        let ok = unsafe { (self.egl.swap_buffers)(self.display, self.surface) };
        if ok == 0 {
            util::log("gpu: eglSwapBuffers failed");
            return false;
        }
        true
    }

    fn draw_scene(
        &mut self,
        tex: u32,
        uv: (f32, f32, f32, f32),
        rect: (i32, i32, u32, u32),
        warmth: u8,
        mode: i32,
        in_size: (f32, f32),
    ) {
        let f = warmth as f32 / 100.0;
        let gain = [1.0 + 0.12 * f, 1.0, 1.0 - 0.40 * f];
        // The DMG panel outside the picture, a touch darker than the light
        // pixel composite so it sits on the same shade as the game.
        let bg = if mode == 5 {
            match self.sys {
                1 => [0.485 * gain[0], 0.502 * gain[1], 0.031 * gain[2]],
                2 => [0.667 * gain[0], 0.710 * gain[1], 0.631 * gain[2]],
                _ => [0.0, 0.0, 0.0],
            }
        } else {
            [0.0, 0.0, 0.0]
        };
        unsafe {
            // The core may leave its own FBO and 3D state behind.
            (self.gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
            (self.gl.viewport)(0, 0, self.w, self.h);
            (self.gl.disable)(GL_DEPTH_TEST);
            (self.gl.disable)(GL_CULL_FACE);
            (self.gl.disable)(GL_STENCIL_TEST);
            (self.gl.disable)(GL_SCISSOR_TEST);
            (self.gl.color_mask)(1, 1, 1, 1);
            (self.gl.clear_color)(bg[0], bg[1], bg[2], 1.0);
            (self.gl.clear)(GL_COLOR_BUFFER_BIT);
            (self.gl.disable)(GL_BLEND);
            (self.gl.use_program)(self.program);
            (self.gl.uniform_2f)(self.u_target, self.w as f32, self.h as f32);
            (self.gl.uniform_4f)(
                self.u_rect,
                rect.0 as f32,
                rect.1 as f32,
                rect.2 as f32,
                rect.3 as f32,
            );
            (self.gl.uniform_3f)(self.u_gain, gain[0], gain[1], gain[2]);
            (self.gl.uniform_1f)(self.u_alpha, 1.0);
            (self.gl.uniform_1f)(self.u_overlay, 0.0);
            (self.gl.uniform_4f)(self.u_uv, uv.0, uv.1, uv.2, uv.3);
            (self.gl.uniform_1i)(self.u_mode, mode);
            (self.gl.uniform_2f)(self.u_in, in_size.0, in_size.1);
            (self.gl.uniform_2f)(self.u_out, rect.2.max(1) as f32, rect.3.max(1) as f32);
            (self.gl.active_texture)(GL_TEXTURE0);
            (self.gl.bind_texture)(GL_TEXTURE_2D, tex);
            self.draw();
            if self.has_overlay {
                (self.gl.enable)(GL_BLEND);
                (self.gl.blend_func)(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
                (self.gl.uniform_4f)(self.u_rect, 0.0, 0.0, self.w as f32, self.h as f32);
                (self.gl.uniform_1f)(self.u_overlay, 1.0);
                (self.gl.uniform_1i)(self.u_mode, 0);
                (self.gl.uniform_4f)(self.u_uv, 0.0, 0.0, 1.0, 1.0);
                (self.gl.bind_texture)(GL_TEXTURE_2D, self.overlay_tex);
                self.draw();
            }
            // The UI layer (the overlays and the menu) rides on top of the
            // game and the bezel: its own alpha decides what shows through.
            if let Some((x, y, w, h)) = self.ui_rect {
                let (tw, th) = self.ui_size;
                if tw > 0 && th > 0 {
                    (self.gl.enable)(GL_BLEND);
                    (self.gl.blend_func)(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
                    (self.gl.uniform_4f)(self.u_rect, x as f32, y as f32, w as f32, h as f32);
                    (self.gl.uniform_4f)(
                        self.u_uv,
                        x as f32 / tw as f32,
                        y as f32 / th as f32,
                        w as f32 / tw as f32,
                        h as f32 / th as f32,
                    );
                    (self.gl.uniform_1i)(self.u_mode, 0);
                    (self.gl.uniform_1f)(self.u_overlay, 1.0);
                    (self.gl.bind_texture)(GL_TEXTURE_2D, self.ui_tex);
                    self.draw();
                }
            }
            (self.gl.disable)(GL_BLEND);
        }
    }

    unsafe fn draw(&self) {
        (self.gl.bind_buffer)(GL_ARRAY_BUFFER, self.vbo);
        // The core may leave its own attribute arrays enabled.
        for i in 1..8 {
            (self.gl.disable_vertex_attrib_array)(i);
        }
        (self.gl.enable_vertex_attrib_array)(0);
        (self.gl.vertex_attrib_pointer)(0, 2, GL_FLOAT, GL_FALSE, 0, ptr::null());
        (self.gl.draw_arrays)(GL_TRIANGLE_STRIP, 0, 4);
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            if let Some(hw) = self.hw.take() {
                self.free_hw(&hw);
            }
            if self.game_tex != 0 {
                (self.gl.delete_textures)(1, &self.game_tex);
            }
            if self.overlay_tex != 0 {
                (self.gl.delete_textures)(1, &self.overlay_tex);
            }
            if self.ui_tex != 0 {
                (self.gl.delete_textures)(1, &self.ui_tex);
            }
            if self.vbo != 0 {
                (self.gl.delete_buffers)(1, &self.vbo);
            }
            if self.program != 0 {
                (self.gl.delete_program)(self.program);
            }
            (self.egl.make_current)(
                self.display,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            (self.egl.destroy_surface)(self.display, self.surface);
            (self.egl.destroy_context)(self.display, self.context);
            (self.egl.terminate)(self.display);
        }
    }
}

unsafe fn uniform(gl: &Gles, program: u32, name: &str) -> i32 {
    let Ok(name) = CString::new(name) else {
        return -1;
    };
    (gl.get_uniform_location)(program, name.as_ptr())
}

unsafe fn make_program(gl: &Gles, vert: &str, frag: &str) -> Result<u32, String> {
    let vs = unsafe { compile(gl, GL_VERTEX_SHADER, vert)? };
    let fs = match unsafe { compile(gl, GL_FRAGMENT_SHADER, frag) } {
        Ok(fs) => fs,
        Err(e) => {
            (gl.delete_shader)(vs);
            return Err(e);
        }
    };
    let program = (gl.create_program)();
    (gl.attach_shader)(program, vs);
    (gl.attach_shader)(program, fs);
    let attr = CString::new("a_pos").unwrap();
    (gl.bind_attrib_location)(program, 0, attr.as_ptr());
    (gl.link_program)(program);
    (gl.delete_shader)(vs);
    (gl.delete_shader)(fs);
    let mut ok = 0;
    (gl.get_programiv)(program, GL_LINK_STATUS, &mut ok);
    if ok == 0 {
        let log = info_log(gl, program, true);
        (gl.delete_program)(program);
        return Err(format!("gpu: shader link failed: {log}"));
    }
    Ok(program)
}

unsafe fn compile(gl: &Gles, kind: u32, source: &str) -> Result<u32, String> {
    let source = CString::new(source).map_err(|_| "gpu: shader contains NUL".to_string())?;
    let shader = (gl.create_shader)(kind);
    let ptr = source.as_ptr();
    (gl.shader_source)(shader, 1, &ptr, ptr::null());
    (gl.compile_shader)(shader);
    let mut ok = 0;
    (gl.get_shaderiv)(shader, GL_COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let log = info_log(gl, shader, false);
        (gl.delete_shader)(shader);
        return Err(format!("gpu: shader compile failed: {log}"));
    }
    Ok(shader)
}

unsafe fn info_log(gl: &Gles, id: u32, program: bool) -> String {
    let mut len = 0;
    if program {
        (gl.get_programiv)(id, GL_INFO_LOG_LENGTH, &mut len);
    } else {
        (gl.get_shaderiv)(id, GL_INFO_LOG_LENGTH, &mut len);
    }
    if len <= 0 {
        return "no info log".into();
    }
    let mut buf = vec![0u8; len as usize];
    if program {
        (gl.get_program_info_log)(id, len, ptr::null_mut(), buf.as_mut_ptr() as *mut c_char);
    } else {
        (gl.get_shader_info_log)(id, len, ptr::null_mut(), buf.as_mut_ptr() as *mut c_char);
    }
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8_lossy(&buf).into_owned()
}
