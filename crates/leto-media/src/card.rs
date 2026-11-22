//! Pictures drawn by the graphics card, from the planes a decoder gives.
//!
//! Turning planes into pixels on the processor takes about as long as
//! decoding them: 6 ms a picture at 1920 × 1200 and 10 ms at 2400 × 1500 on
//! one core of a recent processor, and the pixels are more than twice the
//! bytes to put on the card. Here the planes go to the card as
//! they are, in two textures, and a small program on the card turns them
//! into pixels as it draws them, with the numbers `convert.rs` uses.
//!
//! It draws with OpenGL 3.1 or OpenGL ES 3, through the context the window
//! draws with. Where the card offers less, [`Card::fits`] says so and the
//! pixels are made on the processor as before.
//!
//! A picture that stayed on the card it was decoded on is pixels there
//! already: it is drawn as it is, from where it is (see `kept.rs`).

use glow::{HasContext as _, PixelUnpackData};

#[cfg(target_os = "android")]
use crate::kept::android::Seen;
#[cfg(windows)]
use crate::kept::windows::Seen;
use crate::{Error, Kept, Planes};

type Gl = glow::Context;
type Program = <Gl as glow::HasContext>::Program;
type Texture = <Gl as glow::HasContext>::Texture;
type VertexArray = <Gl as glow::HasContext>::VertexArray;

/// Places the picture over the whole of where it is drawn. The corners come
/// from the number of each, so there is nothing to put on the card for them.
const VERTEX: &str = r"
out vec2 place;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
    place = vec2(corner.x, 1.0 - corner.y);
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
";

/// BT.601 in limited range, as `convert.rs` has it in 256ths: brightness
/// goes from 16 to 235, and each colour from 16 to 240 around 128.
///
/// A colour serves two pixels across and two down. An encoder puts it where
/// the left one of the two is, and halfway between the two rows: that is
/// where H.264, H.265 and AV1 have it unless a stream says otherwise, and
/// where Sunshine and Apollo make it. A texture has it in the middle of the
/// two pixels, half a pixel further right. So the colours are read half a
/// pixel to the right of the brightness, which puts each back under the
/// pixels it was made from; read at the same place, every edge of colour
/// would be drawn half a pixel off and smeared over its neighbours.
const FRAGMENT: &str = r"
uniform sampler2D luma;
uniform sampler2D chroma;
in vec2 place;
out vec4 colour;
void main() {
    float y = 1.1641 * (texture(luma, place).r - 16.0 / 255.0);
    vec2 half_a_pixel = vec2(0.5 / float(textureSize(luma, 0).x), 0.0);
    vec2 c = texture(chroma, place + half_a_pixel).rg - 128.0 / 255.0;
    colour = vec4(
        y + 1.5977 * c.y,
        y - 0.3906 * c.x - 0.8125 * c.y,
        y + 2.0156 * c.x,
        1.0
    );
}
";

/// A picture that is pixels on the card already, drawn as it is.
#[cfg(target_os = "android")]
const FRAGMENT_OF_PIXELS: &str = r"#extension GL_OES_EGL_image_external_essl3 : require
uniform samplerExternalOES picture;
uniform vec4 crop;
in vec2 place;
out vec4 colour;
void main() {
    colour = vec4(texture(picture, crop.xy + place * crop.zw).rgb, 1.0);
}
";

/// A picture that is pixels on the card already, drawn as it is.
#[cfg(windows)]
const FRAGMENT_OF_PIXELS: &str = r"
uniform sampler2D picture;
in vec2 place;
out vec4 colour;
void main() {
    colour = vec4(texture(picture, place).rgb, 1.0);
}
";

/// What is on the graphics card to draw pictures with.
pub struct Card {
    program: Program,
    corners: VertexArray,
    /// One byte of brightness a pixel.
    luma: Texture,
    /// Two bytes of colour for each square of four pixels.
    chroma: Texture,
    /// Width and height of the picture the textures were made for.
    size: Option<(usize, usize)>,
    /// Draws a picture that stayed on the card.
    #[cfg(windows)]
    of_pixels: Program,
    /// Draws a picture that stayed on the card, where the card can.
    #[cfg(target_os = "android")]
    of_pixels: Option<Program>,
    /// Where pictures stay on the card, as OpenGL sees it, once one did.
    #[cfg(any(windows, target_os = "android"))]
    seen: Option<Seen>,
    /// The picture shown last stayed on the card.
    #[cfg(any(windows, target_os = "android"))]
    stays: bool,
}

impl Card {
    /// Whether the card, as `gl` reaches it, draws with a [`Card`].
    pub fn fits(gl: &Gl) -> bool {
        let version = gl.version();
        if version.is_embedded {
            version.major >= 3
        } else {
            (version.major, version.minor) >= (3, 1)
        }
    }

    /// Puts what it draws with on the card. Called on the thread that
    /// draws, with the context current, as every other call.
    pub fn new(gl: &Gl) -> Result<Self, Error> {
        if !Self::fits(gl) {
            return Err(Error::Video("the graphics card draws no pictures from planes".into()));
        }
        #[cfg(windows)]
        let of_pixels = program(gl, FRAGMENT_OF_PIXELS)?;
        // A card without pictures from elsewhere draws the planes alone.
        #[cfg(target_os = "android")]
        let of_pixels = if crate::kept::android::shares(gl) {
            program(gl, FRAGMENT_OF_PIXELS).ok()
        } else {
            None
        };
        let program = program(gl, FRAGMENT)?;
        // SAFETY: plain calls into the context, which is current on this
        // thread, with names it gave.
        unsafe {
            let failed = |what: &str, error: String| Error::Video(format!("{what}: {error}"));
            let corners = gl.create_vertex_array().map_err(|error| failed("no corners", error))?;
            let luma = texture(gl).map_err(|error| failed("no texture", error))?;
            let chroma = texture(gl).map_err(|error| failed("no texture", error))?;
            gl.use_program(Some(program));
            gl.uniform_1_i32(gl.get_uniform_location(program, "luma").as_ref(), 0);
            gl.uniform_1_i32(gl.get_uniform_location(program, "chroma").as_ref(), 1);
            #[cfg(windows)]
            {
                gl.use_program(Some(of_pixels));
                gl.uniform_1_i32(gl.get_uniform_location(of_pixels, "picture").as_ref(), 0);
            }
            #[cfg(target_os = "android")]
            if let Some(of_pixels) = of_pixels {
                gl.use_program(Some(of_pixels));
                gl.uniform_1_i32(gl.get_uniform_location(of_pixels, "picture").as_ref(), 0);
            }
            gl.use_program(None);
            Ok(Self {
                program,
                corners,
                luma,
                chroma,
                size: None,
                #[cfg(any(windows, target_os = "android"))]
                of_pixels,
                #[cfg(any(windows, target_os = "android"))]
                seen: None,
                #[cfg(any(windows, target_os = "android"))]
                stays: false,
            })
        }
    }

    /// Whether a card reached through `gl` draws pictures that stayed on
    /// the card they were decoded on: its driver shares them with Direct3D.
    /// Called on the thread that draws, with the context current. Only on
    /// Windows, and a picture can still fail to be shown: one decoded on
    /// another card than the window is drawn by.
    pub fn keeps(gl: &Gl) -> bool {
        #[cfg(windows)]
        // SAFETY: the context is current on this thread, as every call here
        // is made.
        let keeps = Self::fits(gl) && unsafe { crate::kept::windows::shares() };
        #[cfg(target_os = "android")]
        let keeps = Self::fits(gl) && crate::kept::android::shares(gl);
        #[cfg(not(any(windows, target_os = "android")))]
        let keeps = {
            let _ = gl;
            false
        };
        keeps
    }

    /// Puts `planes` on the card: they are what [`Self::draw`] draws from
    /// now on. A picture that stayed on the card is found there, which can
    /// fail: what was shown before then stays.
    pub fn show(&mut self, gl: &Gl, planes: &Planes) -> Result<(), Error> {
        if let Some(kept) = &planes.kept {
            return self.show_kept(gl, kept);
        }
        let (width, height) = (planes.width, planes.height);
        let (colour_width, colour_height) = planes.chroma_size();
        let fits = width > 0
            && planes.stride >= width
            && planes.luma.len() >= planes.stride * height
            && planes.chroma.len() >= colour_width * 2 * colour_height;
        let number = |count: usize| i32::try_from(count).ok();
        let (Some(across), Some(down), Some(stride), Some(colour_across), Some(colour_down)) = (
            number(width),
            number(height),
            number(planes.stride),
            number(colour_width),
            number(colour_height),
        ) else {
            return Ok(());
        };
        if !fits {
            return Ok(());
        }
        let new_size = self.size != Some((width, height));
        // SAFETY: plain calls into the context, current on this thread, with
        // textures it gave, and planes as large as the rows read from them:
        // `stride` bytes a row of brightness, two bytes a pixel of colour.
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for (texture, plane, size, row, format, inner) in [
                (self.luma, &planes.luma, (across, down), stride, glow::RED, glow::R8),
                (self.chroma, &planes.chroma, (colour_across, colour_down), 0, glow::RG, glow::RG8),
            ] {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, row);
                let pixels = PixelUnpackData::Slice(Some(plane));
                if new_size {
                    let inner = inner as i32;
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        inner,
                        size.0,
                        size.1,
                        0,
                        format,
                        glow::UNSIGNED_BYTE,
                        pixels,
                    );
                } else {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        size.0,
                        size.1,
                        format,
                        glow::UNSIGNED_BYTE,
                        pixels,
                    );
                }
            }
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        self.size = Some((width, height));
        #[cfg(any(windows, target_os = "android"))]
        {
            self.stays = false;
        }
        Ok(())
    }

    /// Finds `kept` on the card: it is what [`Self::draw`] draws from now
    /// on.
    #[cfg(windows)]
    fn show_kept(&mut self, gl: &Gl, kept: &Kept) -> Result<(), Error> {
        if self.seen.as_ref().is_none_or(|seen| !seen.shows(kept)) {
            // SAFETY: the context is current on this thread, and is the one
            // the texture left was opened with.
            unsafe {
                if let Some(seen) = self.seen.take() {
                    seen.close(gl);
                }
                self.seen = Some(Seen::open(gl, kept).map_err(Error::Video)?);
            }
        }
        if let Some(seen) = &mut self.seen {
            seen.take().map_err(Error::Video)?;
        }
        self.stays = true;
        Ok(())
    }

    /// Finds `kept` on the card: it is what [`Self::draw`] draws from now
    /// on.
    #[cfg(target_os = "android")]
    fn show_kept(&mut self, gl: &Gl, kept: &Kept) -> Result<(), Error> {
        if self.of_pixels.is_none() {
            return Err(Error::Video("the card draws no pictures from the decoder".into()));
        }
        // SAFETY: the context is current on this thread, and is the one
        // the texture left was opened with.
        unsafe {
            if self.seen.as_ref().is_none_or(|seen| !seen.shows(kept)) {
                if let Some(seen) = self.seen.take() {
                    seen.close(gl);
                }
                self.seen = Some(Seen::open(gl, kept).map_err(Error::Video)?);
            }
            if let Some(seen) = &mut self.seen {
                seen.take(gl).map_err(Error::Video)?;
            }
        }
        self.stays = true;
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "android")))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
    fn show_kept(&mut self, _gl: &Gl, kept: &Kept) -> Result<(), Error> {
        kept.never()
    }

    /// Draws the picture shown last over the whole of the viewport, or
    /// nothing before the first. Drawing a picture that stayed on the card
    /// can fail, and then draws nothing.
    pub fn draw(&mut self, gl: &Gl) -> Result<(), Error> {
        #[cfg(windows)]
        if self.stays {
            let (program, corners) = (self.of_pixels, self.corners);
            let Some(seen) = &mut self.seen else { return Ok(()) };
            // SAFETY: plain calls into the context, current on this thread,
            // with names it gave: the one the texture was opened with.
            return unsafe {
                seen.draw(gl, |texture| {
                    gl.use_program(Some(program));
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.bind_vertex_array(Some(corners));
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                    gl.bind_vertex_array(None);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    gl.use_program(None);
                })
            }
            .map_err(Error::Video);
        }
        #[cfg(target_os = "android")]
        if self.stays {
            let (Some(program), Some((texture, crop))) =
                (self.of_pixels, self.seen.as_ref().and_then(Seen::drawn))
            else {
                return Ok(());
            };
            // SAFETY: plain calls into the context, current on this thread,
            // with names it gave: the one the texture was opened with.
            unsafe {
                gl.use_program(Some(program));
                gl.uniform_4_f32(
                    gl.get_uniform_location(program, "crop").as_ref(),
                    crop[0],
                    crop[1],
                    crop[2],
                    crop[3],
                );
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(crate::kept::android::TEXTURE_EXTERNAL, Some(texture));
                gl.bind_vertex_array(Some(self.corners));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                gl.bind_vertex_array(None);
                gl.bind_texture(crate::kept::android::TEXTURE_EXTERNAL, None);
                gl.use_program(None);
            }
            return Ok(());
        }
        if self.size.is_none() {
            return Ok(());
        }
        // SAFETY: plain calls into the context, current on this thread, with
        // names it gave.
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.chroma));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.luma));
            gl.bind_vertex_array(Some(self.corners));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
        Ok(())
    }

    /// Waits until the card did everything it was asked before, putting
    /// the last drawing on the screen included where its driver counts
    /// that. With V-Sync, a drawing then never waits behind others for the
    /// screen: only the one being shown is ahead of it.
    pub fn wait(gl: &Gl) {
        // SAFETY: a plain call into the context, current on this thread.
        unsafe { gl.finish() };
    }

    /// Takes off the card what it drew with. Called with the context it
    /// was made with.
    pub fn destroy(self, gl: &Gl) {
        // SAFETY: names the context gave, each deleted once.
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.corners);
            gl.delete_texture(self.luma);
            gl.delete_texture(self.chroma);
            #[cfg(windows)]
            {
                gl.delete_program(self.of_pixels);
                if let Some(seen) = self.seen {
                    seen.close(gl);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(of_pixels) = self.of_pixels {
                    gl.delete_program(of_pixels);
                }
                if let Some(seen) = self.seen {
                    seen.close(gl);
                }
            }
        }
    }
}

/// A texture that is read between its pixels, and up to its edges.
///
/// # Safety
///
/// The context is current on this thread.
unsafe fn texture(gl: &Gl) -> Result<Texture, String> {
    // SAFETY: as the caller promises, with the texture just made.
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        for (what, how) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, what, how as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(texture)
    }
}

/// A program that draws a picture as `fragment` says, in the language
/// version the context speaks.
fn program(gl: &Gl, fragment: &str) -> Result<Program, Error> {
    // An extension is asked for before anything else but the version.
    let (extension, fragment) = match fragment.strip_prefix("#extension") {
        Some(rest) => rest.split_once('\n').map_or(("", fragment), |(line, after)| (line, after)),
        None => ("", fragment),
    };
    let first = if gl.version().is_embedded {
        if extension.is_empty() {
            "#version 300 es\nprecision highp float;\n".to_owned()
        } else {
            format!("#version 300 es\n#extension{extension}\nprecision highp float;\n")
        }
    } else {
        "#version 140\n".to_owned()
    };
    let failed = |what: &str, error: String| Error::Video(format!("{what}: {error}"));
    // SAFETY: plain calls into the context, current on this thread, with
    // names it gave, each deleted once.
    unsafe {
        let program = gl.create_program().map_err(|error| failed("no program", error))?;
        let mut shaders = Vec::new();
        for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(kind).map_err(|error| failed("no shader", error))?;
            gl.shader_source(shader, &format!("{first}{source}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let said = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in shaders {
                    gl.delete_shader(shader);
                }
                gl.delete_program(program);
                return Err(failed("the card did not take the program", said));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        if !gl.version().is_embedded {
            // What the drawing is: the only thing it gives.
            gl.bind_frag_data_location(program, 0, "colour");
        }
        gl.link_program(program);
        let linked = gl.get_program_link_status(program);
        let said = if linked { String::new() } else { gl.get_program_info_log(program) };
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !linked {
            gl.delete_program(program);
            return Err(failed("the card did not link the program", said));
        }
        Ok(program)
    }
}
