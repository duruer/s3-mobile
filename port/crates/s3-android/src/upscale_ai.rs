//! "AI" display filter: Anime4K's small CNN x2 upscaler (`crate::anime4k`, MIT) run on the GPU per new game frame
//! (docs/porting/display.md "AI upscaling filter").
//!
//! Needs a GLES 3 context and renderable half-float textures. Four convolution passes at the game frame's size
//! (RGBA16F, ping-pong `a` / `b`), then depth-to-space into an RGBA8 texture twice the frame size = bilinear frame
//! + the network's residual. The presenter then draws that texture instead of the game frame (same normalized
//! texture coordinates, so menu layouts / the Classic panel split quads work unchanged).

use crate::anime4k;
use crate::gl::*;

struct Pass {
    id: GLuint,
    u_tex: GLint,
    u_pt: GLint,
}

pub struct AiUpscaler {
    conv: Vec<Pass>,
    d2s: GLuint,
    d2s_last: GLint,
    d2s_main: GLint,
    d2s_size: GLint,
    fbo: GLuint,
    a: GLuint,
    b: GLuint,
    out: GLuint,
    size: (u32, u32),
}

unsafe fn tex(w: u32, h: u32, half: bool) -> GLuint {
    let mut t = 0;
    glGenTextures(1, &mut t);
    glBindTexture(GL_TEXTURE_2D, t);
    if half {
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA16F as GLint, w as i32, h as i32, 0, GL_RGBA, GL_HALF_FLOAT, std::ptr::null());
    } else {
        glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8 as GLint, w as i32, h as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
    }
    let f = if half { GL_NEAREST } else { GL_LINEAR };
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, f);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, f);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    t
}

impl AiUpscaler {
    /// Compile the passes (GLES 3 context required).
    pub unsafe fn new() -> Result<AiUpscaler, String> {
        let mut conv = Vec::new();
        for fs in anime4k::CONV {
            let id = program_es3(fs)?;
            conv.push(Pass { id, u_tex: uniform(id, c"u_tex"), u_pt: uniform(id, c"u_pt") });
        }
        let d2s = program_es3(anime4k::DEPTH_TO_SPACE)?;
        let mut fbo = 0;
        glGenFramebuffers(1, &mut fbo);
        Ok(AiUpscaler {
            conv,
            d2s,
            d2s_last: uniform(d2s, c"u_last"),
            d2s_main: uniform(d2s, c"u_main"),
            d2s_size: uniform(d2s, c"u_last_size"),
            fbo,
            a: 0,
            b: 0,
            out: 0,
            size: (0, 0),
        })
    }

    unsafe fn ensure(&mut self, w: u32, h: u32) -> Result<(), String> {
        if self.size == (w, h) {
            return Ok(());
        }
        for t in [self.a, self.b, self.out] {
            if t != 0 {
                glDeleteTextures(1, &t);
            }
        }
        self.a = tex(w, h, true);
        self.b = tex(w, h, true);
        self.out = tex(2 * w, 2 * h, false);
        self.size = (w, h);
        glBindFramebuffer(GL_FRAMEBUFFER, self.fbo);
        for t in [self.a, self.out] {
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, t, 0);
            let st = glCheckFramebufferStatus(GL_FRAMEBUFFER);
            if st != GL_FRAMEBUFFER_COMPLETE {
                glBindFramebuffer(GL_FRAMEBUFFER, 0);
                self.size = (0, 0);
                return Err(format!("framebuffer incomplete: {st:#x}"));
            }
        }
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        Ok(())
    }

    /// Run the network on the game frame texture `main` (`w` x `h`); returns the 2x output texture (RGBA8,
    /// linear filtering). Leaves the default framebuffer bound; the caller sets its viewport / program again.
    pub unsafe fn run(&mut self, main: GLuint, w: u32, h: u32) -> Result<GLuint, String> {
        self.ensure(w, h)?;
        glBindFramebuffer(GL_FRAMEBUFFER, self.fbo);
        glDisable(GL_BLEND);
        glActiveTexture(GL_TEXTURE0);
        let (pw, ph) = (1.0 / w as f32, 1.0 / h as f32);
        glViewport(0, 0, w as i32, h as i32);
        // main -> a -> b -> a -> b
        let mut src = main;
        for (k, p) in self.conv.iter().enumerate() {
            let dst = if k % 2 == 0 { self.a } else { self.b };
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, dst, 0);
            glUseProgram(p.id);
            glBindTexture(GL_TEXTURE_2D, src);
            if k == 0 {
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            }
            glUniform1i(p.u_tex, 0);
            glUniform2f(p.u_pt, pw, ph);
            glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
            src = dst;
        }
        // Depth to space at 2x: residual (last pass, nearest) + bilinear game frame.
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, self.out, 0);
        glViewport(0, 0, 2 * w as i32, 2 * h as i32);
        glUseProgram(self.d2s);
        glActiveTexture(GL_TEXTURE1);
        glBindTexture(GL_TEXTURE_2D, main);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, src);
        glUniform1i(self.d2s_last, 0);
        glUniform1i(self.d2s_main, 1);
        glUniform2f(self.d2s_size, w as f32, h as f32);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        Ok(self.out)
    }
}
