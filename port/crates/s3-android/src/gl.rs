//! Minimal EGL + GLES 2.0 bindings (only what the presenter uses) and a small textured-quad renderer.
//! Linked against the NDK's libEGL.so / libGLESv2.so (always present on Android). See docs/porting/display.md.
#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

pub type EGLDisplay = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLint = i32;
pub type EGLBoolean = c_uint;

pub const EGL_NO_DISPLAY: EGLDisplay = std::ptr::null_mut();
pub const EGL_NO_CONTEXT: EGLContext = std::ptr::null_mut();
pub const EGL_NO_SURFACE: EGLSurface = std::ptr::null_mut();
pub const EGL_DEFAULT_DISPLAY: *mut c_void = std::ptr::null_mut();
pub const EGL_NONE: EGLint = 0x3038;
pub const EGL_RED_SIZE: EGLint = 0x3024;
pub const EGL_GREEN_SIZE: EGLint = 0x3023;
pub const EGL_BLUE_SIZE: EGLint = 0x3022;
pub const EGL_ALPHA_SIZE: EGLint = 0x3021;
pub const EGL_DEPTH_SIZE: EGLint = 0x3025;
pub const EGL_SURFACE_TYPE: EGLint = 0x3033;
pub const EGL_WINDOW_BIT: EGLint = 0x0004;
pub const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
pub const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
pub const EGL_NATIVE_VISUAL_ID: EGLint = 0x302E;
pub const EGL_CONTEXT_CLIENT_VERSION: EGLint = 0x3098;
pub const EGL_WIDTH: EGLint = 0x3057;
pub const EGL_HEIGHT: EGLint = 0x3056;
pub const EGL_BAD_SURFACE: EGLint = 0x300D;
pub const EGL_CONTEXT_LOST: EGLint = 0x300E;

#[cfg_attr(target_os = "android", link(name = "EGL"))]
extern "C" {
    pub fn eglGetDisplay(d: *mut c_void) -> EGLDisplay;
    pub fn eglInitialize(d: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean;
    pub fn eglTerminate(d: EGLDisplay) -> EGLBoolean;
    pub fn eglChooseConfig(d: EGLDisplay, attribs: *const EGLint, configs: *mut EGLConfig, size: EGLint, n: *mut EGLint) -> EGLBoolean;
    pub fn eglGetConfigAttrib(d: EGLDisplay, c: EGLConfig, attr: EGLint, v: *mut EGLint) -> EGLBoolean;
    pub fn eglCreateContext(d: EGLDisplay, c: EGLConfig, share: EGLContext, attribs: *const EGLint) -> EGLContext;
    pub fn eglDestroyContext(d: EGLDisplay, c: EGLContext) -> EGLBoolean;
    pub fn eglCreateWindowSurface(d: EGLDisplay, c: EGLConfig, win: *mut c_void, attribs: *const EGLint) -> EGLSurface;
    pub fn eglDestroySurface(d: EGLDisplay, s: EGLSurface) -> EGLBoolean;
    pub fn eglMakeCurrent(d: EGLDisplay, draw: EGLSurface, read: EGLSurface, c: EGLContext) -> EGLBoolean;
    pub fn eglSwapBuffers(d: EGLDisplay, s: EGLSurface) -> EGLBoolean;
    pub fn eglSwapInterval(d: EGLDisplay, interval: EGLint) -> EGLBoolean;
    pub fn eglQuerySurface(d: EGLDisplay, s: EGLSurface, attr: EGLint, v: *mut EGLint) -> EGLBoolean;
    pub fn eglGetError() -> EGLint;
}

pub type GLenum = c_uint;
pub type GLuint = c_uint;
pub type GLint = c_int;
pub type GLsizei = c_int;
pub type GLfloat = f32;

pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_TEXTURE0: GLenum = 0x84C0;
pub const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
pub const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
pub const GL_NEAREST: GLint = 0x2600;
pub const GL_LINEAR: GLint = 0x2601;
pub const GL_CLAMP_TO_EDGE: GLint = 0x812F;
pub const GL_RGB: GLenum = 0x1907;
pub const GL_RGBA: GLenum = 0x1908;
pub const GL_UNSIGNED_BYTE: GLenum = 0x1401;
pub const GL_UNSIGNED_SHORT_5_6_5: GLenum = 0x8363;
pub const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
pub const GL_COLOR_BUFFER_BIT: GLenum = 0x4000;
pub const GL_TRIANGLE_STRIP: GLenum = 0x0005;
pub const GL_FLOAT: GLenum = 0x1406;
pub const GL_VERTEX_SHADER: GLenum = 0x8B31;
pub const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
pub const GL_COMPILE_STATUS: GLenum = 0x8B81;
pub const GL_LINK_STATUS: GLenum = 0x8B82;
pub const GL_BLEND: GLenum = 0x0BE2;
pub const GL_SRC_ALPHA: GLenum = 0x0302;
pub const GL_ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
pub const GL_SCISSOR_TEST: GLenum = 0x0C11;
pub const GL_DITHER: GLenum = 0x0BD0;

#[cfg_attr(target_os = "android", link(name = "GLESv2"))]
extern "C" {
    pub fn glGenTextures(n: GLsizei, t: *mut GLuint);
    pub fn glDeleteTextures(n: GLsizei, t: *const GLuint);
    pub fn glBindTexture(target: GLenum, t: GLuint);
    pub fn glActiveTexture(t: GLenum);
    pub fn glTexParameteri(target: GLenum, p: GLenum, v: GLint);
    pub fn glTexImage2D(target: GLenum, level: GLint, ifmt: GLint, w: GLsizei, h: GLsizei, border: GLint, fmt: GLenum, ty: GLenum, data: *const c_void);
    pub fn glTexSubImage2D(target: GLenum, level: GLint, x: GLint, y: GLint, w: GLsizei, h: GLsizei, fmt: GLenum, ty: GLenum, data: *const c_void);
    pub fn glPixelStorei(p: GLenum, v: GLint);
    pub fn glViewport(x: GLint, y: GLint, w: GLsizei, h: GLsizei);
    pub fn glClearColor(r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat);
    pub fn glClear(mask: GLenum);
    pub fn glEnable(cap: GLenum);
    pub fn glDisable(cap: GLenum);
    pub fn glBlendFunc(s: GLenum, d: GLenum);
    pub fn glCreateShader(ty: GLenum) -> GLuint;
    pub fn glShaderSource(s: GLuint, n: GLsizei, src: *const *const c_char, len: *const GLint);
    pub fn glCompileShader(s: GLuint);
    pub fn glGetShaderiv(s: GLuint, p: GLenum, v: *mut GLint);
    pub fn glGetShaderInfoLog(s: GLuint, max: GLsizei, len: *mut GLsizei, log: *mut c_char);
    pub fn glDeleteShader(s: GLuint);
    pub fn glCreateProgram() -> GLuint;
    pub fn glAttachShader(p: GLuint, s: GLuint);
    pub fn glBindAttribLocation(p: GLuint, i: GLuint, name: *const c_char);
    pub fn glLinkProgram(p: GLuint);
    pub fn glGetProgramiv(p: GLuint, pn: GLenum, v: *mut GLint);
    pub fn glGetProgramInfoLog(p: GLuint, max: GLsizei, len: *mut GLsizei, log: *mut c_char);
    pub fn glUseProgram(p: GLuint);
    pub fn glDeleteProgram(p: GLuint);
    pub fn glGetUniformLocation(p: GLuint, name: *const c_char) -> GLint;
    pub fn glUniform1i(l: GLint, v: GLint);
    pub fn glUniform2f(l: GLint, a: GLfloat, b: GLfloat);
    pub fn glUniform4f(l: GLint, a: GLfloat, b: GLfloat, c: GLfloat, d: GLfloat);
    pub fn glEnableVertexAttribArray(i: GLuint);
    pub fn glVertexAttribPointer(i: GLuint, size: GLint, ty: GLenum, norm: u8, stride: GLsizei, p: *const c_void);
    pub fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);
    pub fn glGetError() -> GLenum;
    pub fn glFinish();
}

/// Quad vertex shader: `u_rect` = destination (x0, y0, x1, y1) in clip space; `u_uv` = source (u0, v0, u1, v1)
/// in texture coordinates (0, 0, 1, 1 = whole texture; sub-rectangles for menu layout pieces).
pub const VS: &str = "attribute vec2 a_pos;\n\
varying vec2 v_uv;\n\
uniform vec4 u_rect;\n\
uniform vec4 u_uv;\n\
void main() {\n\
  v_uv = mix(u_uv.xy, u_uv.zw, a_pos);\n\
  gl_Position = vec4(mix(u_rect.xy, u_rect.zw, a_pos), 0.0, 1.0);\n\
}\n";

/// Plain sampling (nearest or linear by texture parameters); also used for the RGBA button sprites.
pub const FS_PLAIN: &str = "precision mediump float;\n\
varying vec2 v_uv;\n\
uniform sampler2D u_tex;\n\
void main() { gl_FragColor = texture2D(u_tex, v_uv); }\n";

/// Sharp bilinear (texture uses GL_LINEAR): nearest inside a source texel, a one-screen-pixel linear band at the
/// texel edges. `u_size` = texture size in texels, `u_scale` = screen pixels per texel (>= 1).
pub const FS_SHARP: &str = "precision highp float;\n\
varying vec2 v_uv;\n\
uniform sampler2D u_tex;\n\
uniform vec2 u_size;\n\
uniform vec2 u_scale;\n\
void main() {\n\
  vec2 texel = v_uv * u_size;\n\
  vec2 fl = floor(texel);\n\
  vec2 c = fract(texel) - 0.5;\n\
  vec2 range = 0.5 - 0.5 / u_scale;\n\
  vec2 f = (c - clamp(c, -range, range)) * u_scale + 0.5;\n\
  gl_FragColor = texture2D(u_tex, (fl + f) / u_size);\n\
}\n";

unsafe fn compile(ty: GLenum, src: &str) -> Result<GLuint, String> {
    let s = glCreateShader(ty);
    let c = std::ffi::CString::new(src).unwrap();
    let p = c.as_ptr();
    glShaderSource(s, 1, &p, std::ptr::null());
    glCompileShader(s);
    let mut ok = 0;
    glGetShaderiv(s, GL_COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = vec![0 as c_char; 1024];
        let mut n = 0;
        glGetShaderInfoLog(s, 1024, &mut n, buf.as_mut_ptr());
        glDeleteShader(s);
        return Err(format!("shader: {}", CStr::from_ptr(buf.as_ptr()).to_string_lossy()));
    }
    Ok(s)
}

/// A linked program with the uniforms the presenter sets.
pub struct Program {
    pub id: GLuint,
    pub u_rect: GLint,
    pub u_uv: GLint,
    pub u_tex: GLint,
    pub u_size: GLint,
    pub u_scale: GLint,
}

pub unsafe fn program(fs: &str) -> Result<Program, String> {
    let v = compile(GL_VERTEX_SHADER, VS)?;
    let f = compile(GL_FRAGMENT_SHADER, fs)?;
    let p = glCreateProgram();
    glAttachShader(p, v);
    glAttachShader(p, f);
    glBindAttribLocation(p, 0, c"a_pos".as_ptr());
    glLinkProgram(p);
    glDeleteShader(v);
    glDeleteShader(f);
    let mut ok = 0;
    glGetProgramiv(p, GL_LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = vec![0 as c_char; 1024];
        let mut n = 0;
        glGetProgramInfoLog(p, 1024, &mut n, buf.as_mut_ptr());
        glDeleteProgram(p);
        return Err(format!("link: {}", CStr::from_ptr(buf.as_ptr()).to_string_lossy()));
    }
    Ok(Program {
        id: p,
        u_rect: glGetUniformLocation(p, c"u_rect".as_ptr()),
        u_uv: glGetUniformLocation(p, c"u_uv".as_ptr()),
        u_tex: glGetUniformLocation(p, c"u_tex".as_ptr()),
        u_size: glGetUniformLocation(p, c"u_size".as_ptr()),
        u_scale: glGetUniformLocation(p, c"u_scale".as_ptr()),
    })
}

/// Unit quad (triangle strip), client-side array.
pub static QUAD: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];

/// Window-pixel rectangle -> clip-space (x0, y0, x1, y1) with uv (0,0) at the top-left of the rectangle.
pub fn clip_rect(x: f32, y: f32, w: f32, h: f32, win_w: f32, win_h: f32) -> [f32; 4] {
    let x0 = x / win_w * 2.0 - 1.0;
    let x1 = (x + w) / win_w * 2.0 - 1.0;
    let y0 = 1.0 - y / win_h * 2.0;
    let y1 = 1.0 - (y + h) / win_h * 2.0;
    [x0, y0, x1, y1]
}

#[cfg(test)]
mod tests {
    #[test]
    fn clip_rect_maps_window_to_ndc() {
        assert_eq!(super::clip_rect(0.0, 0.0, 100.0, 50.0, 100.0, 50.0), [-1.0, 1.0, 1.0, -1.0]);
        assert_eq!(super::clip_rect(50.0, 25.0, 50.0, 25.0, 100.0, 50.0), [0.0, 0.0, 1.0, -1.0]);
    }
}
