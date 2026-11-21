//! Pictures that go straight from the decoder to the screen.
//!
//! The decoder's pictures are Direct3D's. Drawn by the window's OpenGL they
//! cross from one to the other, and wait for the window's next drawing.
//! Here the card that decoded a picture draws it itself, at once, on a pane
//! of its own inside Leto's window, and shows it: no OpenGL, and nothing of
//! the window's own pace.
//!
//! It is done the way Moonlight does it on Windows (`d3d11va.cpp` of
//! moonlight-qt, read for this and not copied): the card copies the picture
//! out of the decoder's textures into one a small program reads, that
//! program turns brightness and colours into pixels as it draws them, and
//! the picture is shown without waiting for the screen. The numbers of the
//! program are those of `convert.rs` and of `card.rs`.
//!
//! Only on Windows.

use std::ffi::c_void;

use windows::{
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::{
            Direct3D::{
                D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D, Fxc::D3DCompile,
                ID3DBlob,
            },
            Direct3D11::{
                D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BUFFER_DESC,
                D3D11_COMPARISON_NEVER, D3D11_CULL_NONE, D3D11_FILL_SOLID,
                D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_FLOAT32_MAX, D3D11_RASTERIZER_DESC,
                D3D11_SAMPLER_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC,
                D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_SRV,
                D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                D3D11_VIEWPORT, ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
                ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView,
                ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
            },
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT, DXGI_FORMAT_NV12,
                    DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
                    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
                },
                DXGI_FEATURE_PRESENT_ALLOW_TEARING, DXGI_MWA_NO_ALT_ENTER,
                DXGI_MWA_NO_WINDOW_CHANGES, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING,
                DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, DXGI_SWAP_EFFECT_FLIP_DISCARD,
                DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGIFactory5,
                IDXGISwapChain1,
            },
        },
        Media::MediaFoundation::IMFSample,
        UI::WindowsAndMessaging::GetClientRect,
    },
    core::{BOOL, Interface as _, PCSTR, s},
};

use crate::kept::windows::texture_of;

/// Places a triangle over the whole of what is drawn into, and more: its
/// corners come from their numbers, so there is nothing to give the card
/// for them. `place` goes from 0 to 1 across and down the part that shows.
const CORNERS: &str = r"
struct Corner {
    float4 at : SV_Position;
    float2 place : TEXCOORD0;
};
Corner main(uint number : SV_VertexID) {
    Corner corner;
    corner.place = float2((number << 1) & 2, number & 2);
    corner.at = float4(corner.place.x * 2.0 - 1.0, 1.0 - corner.place.y * 2.0, 0.0, 1.0);
    return corner;
}
";

/// BT.601 in limited range, with the numbers of `card.rs`: brightness goes
/// from 16 to 235, and each colour from 16 to 240 around 128.
///
/// The decoder's textures are larger than the picture, so only `shown` of
/// them is read, and never past `luma_ends` and `chroma_ends`: beyond them
/// is padding. A colour serves two pixels across, and an encoder puts it
/// where the left one is, which a texture has half a pixel further left
/// than its middle: the colours are read `colour_shift` to the right.
const COLOURS: &str = r"
Texture2D<float> luma : register(t0);
Texture2D<float2> chroma : register(t1);
SamplerState between : register(s0);
cbuffer sizes : register(b0) {
    float2 shown;
    float2 luma_ends;
    float2 chroma_ends;
    float2 colour_shift;
};
float4 main(float4 at : SV_Position, float2 place : TEXCOORD0) : SV_Target {
    float2 spot = place * shown;
    float y = 1.1641 * (luma.Sample(between, min(spot, luma_ends)) - 16.0 / 255.0);
    float2 c = chroma.Sample(between, min(spot + colour_shift, chroma_ends)) - 128.0 / 255.0;
    return float4(
        y + 1.5977 * c.y,
        y - 0.3906 * c.x - 0.8125 * c.y,
        y + 2.0156 * c.x,
        1.0
    );
}
";

/// What the program is told about sizes: `shown`, `luma_ends`,
/// `chroma_ends` and `colour_shift`, two numbers each.
type Sizes = [f32; 8];

/// What the program is told for a picture of `shown` pixels in textures
/// laid out `laid` pixels across and down.
fn sizes(shown: (u32, u32), laid: (u32, u32)) -> Sizes {
    let (width, height) = (shown.0 as f32, shown.1 as f32);
    let (across, down) = (laid.0.max(1) as f32, laid.1.max(1) as f32);
    [
        width / across,
        height / down,
        // The middle of the last pixel of brightness, and of colour, which
        // is two pixels wide and high.
        (width - 0.5) / across,
        (height - 0.5) / down,
        (width - 1.0) / across,
        (height - 1.0) / down,
        0.5 / across,
        0.0,
    ]
}

/// Draws the decoder's pictures on a window and shows them.
pub struct Straight {
    commands: ID3D11DeviceContext,
    /// The card's commands taken for a whole drawing, where Windows has
    /// that: the decoder gives them commands of its own in between.
    turns: Option<ID3D11Multithread>,
    window: HWND,
    chain: IDXGISwapChain1,
    /// What the chain was made with, said again each time it changes size.
    flags: DXGI_SWAP_CHAIN_FLAG,
    /// How a picture is shown: without waiting, torn or not.
    present: DXGI_PRESENT,
    /// What is drawn into, and how large it is. `None` until it is made
    /// for the size the window has.
    target: Option<(ID3D11RenderTargetView, (u32, u32))>,
    corners: ID3D11VertexShader,
    colours: ID3D11PixelShader,
    between: ID3D11SamplerState,
    plain: ID3D11RasterizerState,
    /// What a picture of the current size and layout goes through.
    made: Option<Made>,
}

/// What is made once for pictures of one size, in textures of one layout.
struct Made {
    shown: (u32, u32),
    laid: (u32, u32, DXGI_FORMAT),
    /// The latest picture, in the planes the decoder made.
    picture: ID3D11Texture2D,
    luma: ID3D11ShaderResourceView,
    chroma: ID3D11ShaderResourceView,
    sizes: ID3D11Buffer,
}

// SAFETY: the device was told to protect itself from several threads
// (`SetMultithreadProtected`), which covers what is made from it, and the
// window is only ever asked how large it is, which any thread may ask. A
// `Straight` is used by one thread at a time: its decoder's.
unsafe impl Send for Straight {}

impl Straight {
    /// What draws on `window` with `device`, the card the pictures are
    /// decoded on. With `torn`, a picture is shown at once even in the
    /// middle of a refresh of the screen, where Windows lets it.
    pub fn new(device: &ID3D11Device, window: isize, torn: bool) -> windows::core::Result<Self> {
        let window = HWND(window as *mut c_void);
        // SAFETY: plain calls into the device and what it gives, with
        // descriptions made here in full.
        unsafe {
            let commands = device.GetImmediateContext()?;
            let factory: IDXGIFactory2 = device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let tears = torn && tearing(&factory);
            let flags =
                if tears { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING } else { DXGI_SWAP_CHAIN_FLAG(0) };
            let (width, height) = inside(window).unwrap_or((16, 16));
            let wanted = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                Stereo: BOOL::from(false),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 3,
                Scaling: DXGI_SCALING_STRETCH,
                // The newest picture replaces one not shown yet.
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
                Flags: flags.0 as u32,
            };
            let chain = factory.CreateSwapChainForHwnd(device, window, &wanted, None, None)?;
            // Leto's window decides what Alt+Enter does, not this.
            let _ = factory
                .MakeWindowAssociation(window, DXGI_MWA_NO_WINDOW_CHANGES | DXGI_MWA_NO_ALT_ENTER);

            let corners = compile(CORNERS, s!("vs_4_0"))?;
            let colours = compile(COLOURS, s!("ps_4_0"))?;
            let mut vertex = None;
            device.CreateVertexShader(bytes(&corners), None, Some(&mut vertex))?;
            let mut pixel = None;
            device.CreatePixelShader(bytes(&colours), None, Some(&mut pixel))?;

            // Read between the pixels, and up to the edges.
            let sampler = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxAnisotropy: 1,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: D3D11_FLOAT32_MAX,
                ..Default::default()
            };
            let mut between = None;
            device.CreateSamplerState(&sampler, Some(&mut between))?;
            // Every triangle is drawn, whichever way round.
            let drawn = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: BOOL::from(true),
                ..Default::default()
            };
            let mut plain = None;
            device.CreateRasterizerState(&drawn, Some(&mut plain))?;

            let none = windows::core::Error::empty;
            Ok(Self {
                turns: commands.cast().ok(),
                commands,
                window,
                chain,
                flags,
                present: if tears { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) },
                target: None,
                corners: vertex.ok_or_else(none)?,
                colours: pixel.ok_or_else(none)?,
                between: between.ok_or_else(none)?,
                plain: plain.ok_or_else(none)?,
                made: None,
            })
        }
    }

    /// Whether pictures are shown in the middle of a refresh of the screen.
    pub fn tears(&self) -> bool {
        self.present == DXGI_PRESENT_ALLOW_TEARING
    }

    /// Draws the picture in `sample`, of which `shown` pixels across and
    /// down are to be seen, over the whole of the window, and shows it.
    /// Nothing is waited for.
    pub fn show(
        &mut self,
        device: &ID3D11Device,
        sample: &IMFSample,
        shown: (usize, usize),
    ) -> windows::core::Result<()> {
        let shown = (u32::try_from(shown.0).unwrap_or(0), u32::try_from(shown.1).unwrap_or(0));
        // SAFETY: reading the sample the decoder just gave.
        let (source, slice, laid) = unsafe { texture_of(sample) }?;
        let turns = self.turns.clone();
        let _turn = Turn::take(turns.as_ref());
        if self.draw(device, &source, slice, laid, shown)? {
            // At once: the newest picture replaces one not shown yet.
            // SAFETY: a plain call into the chain, which has its picture.
            unsafe { self.chain.Present(0, self.present) }.ok()?;
        }
        Ok(())
    }

    /// Draws `shown` pixels of the picture in `slice` of `source`, textures
    /// laid out as `laid` says, over the whole of the window, without
    /// showing it yet. `false` when the window has nothing to draw on.
    fn draw(
        &mut self,
        device: &ID3D11Device,
        source: &ID3D11Texture2D,
        slice: u32,
        laid: (u32, u32, DXGI_FORMAT),
        shown: (u32, u32),
    ) -> windows::core::Result<bool> {
        if self.made.as_ref().is_none_or(|made| made.shown != shown || made.laid != laid) {
            self.made = Some(make(device, shown, laid)?);
        }
        let Some(made) = &self.made else {
            return Err(windows::core::Error::empty());
        };
        // SAFETY: the whole of one texture of the decoder's into one made
        // as large, of the same kind, by the same device. The decoder has
        // its texture back at once.
        unsafe {
            self.commands.CopySubresourceRegion(&made.picture, 0, 0, 0, 0, source, slice, None);
        }

        // A window that is gone, or has no size, has nothing to draw on.
        let Some(size) = inside(self.window) else { return Ok(false) };
        if self.target.as_ref().is_none_or(|(_, made_for)| *made_for != size) {
            // SAFETY: nothing of the chain is held while it changes size:
            // what was drawn into is let go of first.
            unsafe {
                self.commands.OMSetRenderTargets(None, None);
                self.target = None;
                // The card lets go of it for good only once told to.
                self.commands.Flush();
                self.chain.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, self.flags)?;
                let back: ID3D11Texture2D = self.chain.GetBuffer(0)?;
                let mut target = None;
                device.CreateRenderTargetView(&back, None, Some(&mut target))?;
                self.target = Some((target.ok_or_else(windows::core::Error::empty)?, size));
            }
        }
        let Some((target, _)) = &self.target else {
            return Err(windows::core::Error::empty());
        };
        let whole = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.0 as f32,
            Height: size.1 as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: plain calls into the card's commands, with what the same
        // device made. All that drawing needs is said each time: showing a
        // picture takes what is drawn into away, and the decoder shares
        // these commands.
        unsafe {
            let commands = &self.commands;
            commands.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            commands.RSSetViewports(Some(&[whole]));
            commands.RSSetState(&self.plain);
            commands.IASetInputLayout(None);
            commands.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            commands.VSSetShader(&self.corners, None);
            commands.PSSetShader(&self.colours, None);
            commands.PSSetConstantBuffers(0, Some(&[Some(made.sizes.clone())]));
            commands.PSSetSamplers(0, Some(&[Some(self.between.clone())]));
            commands.PSSetShaderResources(
                0,
                Some(&[Some(made.luma.clone()), Some(made.chroma.clone())]),
            );
            commands.Draw(3, 0);
            // The next picture is copied into what was just read from.
            commands.PSSetShaderResources(0, Some(&[None, None]));
        }
        Ok(true)
    }
}

/// The card's commands, taken until this is dropped: nothing of another
/// thread's comes between those given meanwhile.
struct Turn<'a>(Option<&'a ID3D11Multithread>);

impl<'a> Turn<'a> {
    fn take(turns: Option<&'a ID3D11Multithread>) -> Self {
        if let Some(turns) = turns {
            // SAFETY: a plain call, answered by `Leave` when this drops.
            unsafe { turns.Enter() };
        }
        Self(turns)
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        if let Some(turns) = self.0 {
            // SAFETY: a plain call, after the `Enter` this was made with.
            unsafe { turns.Leave() };
        }
    }
}

/// Whether Windows lets a picture be shown in the middle of a refresh.
///
/// # Safety
///
/// A plain call into the factory.
unsafe fn tearing(factory: &IDXGIFactory2) -> bool {
    let Ok(newer) = factory.cast::<IDXGIFactory5>() else { return false };
    let mut allowed = BOOL::from(false);
    // SAFETY: as the caller promises, with room for the one answer asked.
    let asked = unsafe {
        newer.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            (&raw mut allowed).cast(),
            size_of::<BOOL>() as u32,
        )
    };
    asked.is_ok() && allowed.as_bool()
}

/// How many pixels the inside of `window` is across and down. `None` for a
/// window with no inside to draw on.
fn inside(window: HWND) -> Option<(u32, u32)> {
    let mut all = RECT::default();
    // SAFETY: asking Windows about a window, which any thread may do; a
    // window that is gone gives an error.
    unsafe { GetClientRect(window, &mut all) }.ok()?;
    let width = u32::try_from(all.right - all.left).ok()?;
    let height = u32::try_from(all.bottom - all.top).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

/// The program in `source`, made for the card by Windows' own compiler.
///
/// # Safety
///
/// `kind` names a kind of program the compiler knows.
unsafe fn compile(source: &str, kind: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut made = None;
    let mut said: Option<ID3DBlob> = None;
    // SAFETY: the source with its length, a function called `main` in it,
    // and room for what comes back.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            s!("main"),
            kind,
            0,
            0,
            &mut made,
            Some(&mut said),
        )
    };
    if let Err(error) = compiled {
        // What the compiler said goes with the error: it names the line.
        return Err(match said {
            // SAFETY: what the compiler said, as long as it says it is.
            Some(said) => windows::core::Error::new(
                error.code(),
                String::from_utf8_lossy(unsafe { bytes(&said) }).trim(),
            ),
            None => error,
        });
    }
    made.ok_or_else(windows::core::Error::empty)
}

/// What is in `blob`.
///
/// # Safety
///
/// The bytes are read while `blob` lives.
unsafe fn bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob says where its bytes are and how many.
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()) }
}

/// The texture pictures are copied into, as large as the decoder's and of
/// their kind, how the program reads its brightness and its colours, and
/// what it is told about sizes.
fn make(
    device: &ID3D11Device,
    shown: (u32, u32),
    laid: (u32, u32, DXGI_FORMAT),
) -> windows::core::Result<Made> {
    if laid.2 != DXGI_FORMAT_NV12 {
        // Ten bits a colour, or colours for every pixel: not drawn here.
        log::debug!("pictures in {:?} do not go straight to the screen", laid.2);
        return Err(windows::core::Error::empty());
    }
    let wanted = D3D11_TEXTURE2D_DESC {
        Width: laid.0,
        Height: laid.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: laid.2,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let told = sizes(shown, (laid.0, laid.1));
    let buffer = D3D11_BUFFER_DESC {
        ByteWidth: size_of::<Sizes>() as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        ..Default::default()
    };
    let first = D3D11_SUBRESOURCE_DATA { pSysMem: told.as_ptr().cast(), ..Default::default() };
    let none = windows::core::Error::empty;
    // SAFETY: plain calls into the device, with descriptions made here in
    // full, and numbers that live until the buffer has them.
    unsafe {
        let mut picture = None;
        device.CreateTexture2D(&wanted, None, Some(&mut picture))?;
        let picture = picture.ok_or_else(none)?;
        // One byte of brightness a pixel, then two of colour for each
        // square of four: each plane is read as a texture of its own.
        let plane = |format| {
            let read = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 },
                },
            };
            let mut view = None;
            device.CreateShaderResourceView(&picture, Some(&read), Some(&mut view))?;
            view.ok_or_else(none)
        };
        let luma = plane(DXGI_FORMAT_R8_UNORM)?;
        let chroma = plane(DXGI_FORMAT_R8G8_UNORM)?;
        let mut sizes = None;
        device.CreateBuffer(&buffer, Some(&first), Some(&mut sizes))?;
        Ok(Made { shown, laid, picture, luma, chroma, sizes: sizes.ok_or_else(none)? })
    }
}

#[cfg(test)]
mod tests {
    use windows::{
        Win32::{
            Foundation::HMODULE,
            Graphics::{
                Direct3D::D3D_DRIVER_TYPE_WARP,
                Direct3D11::{
                    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG,
                    D3D11_FORMAT_SUPPORT_TEXTURE2D, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
                    D3D11_SDK_VERSION, D3D11_USAGE_STAGING, D3D11CreateDevice,
                },
                Dxgi::IDXGIAdapter,
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
                SetWindowPos, WINDOW_EX_STYLE, WS_CHILD, WS_POPUP,
            },
        },
        core::w,
    };

    use super::*;

    /// Windows' own card made of software, which every Windows has, and
    /// which makes pictures of brightness and colours as a real one does.
    fn software_card() -> ID3D11Device {
        let mut device = None;
        // SAFETY: a device asked of Windows with room for it, and a plain
        // question to it.
        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .expect("Windows makes a card of software");
            let device = device.expect("the card of software");
            // A test that drew nothing would say nothing: it fails instead.
            let can = device.CheckFormatSupport(DXGI_FORMAT_NV12).unwrap_or(0);
            assert_ne!(
                can & D3D11_FORMAT_SUPPORT_TEXTURE2D.0 as u32,
                0,
                "the card of software makes no pictures of brightness and colours"
            );
            device
        }
    }

    /// A picture as a decoder lays it out, 64 by 64 of which 64 by 48 show:
    /// red on the left and blue on the right, with green in the padding
    /// underneath, in BT.601's limited range.
    fn picture(device: &ID3D11Device) -> ID3D11Texture2D {
        const RED: [u8; 3] = [81, 90, 240];
        const BLUE: [u8; 3] = [41, 240, 110];
        const GREEN: [u8; 3] = [145, 54, 34];
        let colour = |x: usize, y: usize| match (x < 32, y < 48) {
            (_, false) => GREEN,
            (true, true) => RED,
            (false, true) => BLUE,
        };
        let mut planes = Vec::with_capacity(64 * 96);
        for y in 0..64 {
            planes.extend((0..64).map(|x| colour(x, y)[0]));
        }
        for y in (0..64).step_by(2) {
            for x in (0..64).step_by(2) {
                planes.extend(&colour(x, y)[1..]);
            }
        }
        let wanted = D3D11_TEXTURE2D_DESC {
            Width: 64,
            Height: 64,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let first = D3D11_SUBRESOURCE_DATA {
            pSysMem: planes.as_ptr().cast(),
            SysMemPitch: 64,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        // SAFETY: a texture described in full, with bytes for both of its
        // planes that live until it has them.
        unsafe { device.CreateTexture2D(&wanted, Some(&first), Some(&mut texture)) }
            .expect("a texture with a picture in it");
        texture.expect("the texture")
    }

    /// The pixel `at` of what `straight` drew last, before it is shown.
    fn pixel(device: &ID3D11Device, straight: &Straight, at: (u32, u32)) -> [u8; 3] {
        // SAFETY: what was drawn into is copied to a texture memory reads,
        // and one pixel inside it is read while it is mapped.
        unsafe {
            let back: ID3D11Texture2D = straight.chain.GetBuffer(0).expect("what was drawn into");
            let mut like = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut like);
            assert!(at.0 < like.Width && at.1 < like.Height, "{at:?} in {like:?}");
            like.Usage = D3D11_USAGE_STAGING;
            like.BindFlags = 0;
            like.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            like.MiscFlags = 0;
            let mut read = None;
            device.CreateTexture2D(&like, None, Some(&mut read)).expect("a texture to read");
            let read = read.expect("the texture to read");
            straight.commands.CopyResource(&read, &back);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            straight
                .commands
                .Map(&read, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .expect("the pixels in memory");
            let row = mapped.pData.cast::<u8>().add(at.1 as usize * mapped.RowPitch as usize);
            let pixel = std::slice::from_raw_parts(row.add(at.0 as usize * 4), 3);
            let pixel = [pixel[0], pixel[1], pixel[2]];
            straight.commands.Unmap(&read, 0);
            pixel
        }
    }

    fn close_to(pixel: [u8; 3], wanted: [u8; 3]) -> bool {
        pixel.iter().zip(wanted).all(|(got, wanted)| got.abs_diff(wanted) <= 6)
    }

    #[test]
    fn windows_makes_both_programs_for_the_card() {
        // SAFETY: the two kinds of program the compiler is asked for.
        unsafe {
            compile(CORNERS, s!("vs_4_0")).expect("the program that places the corners");
            compile(COLOURS, s!("ps_4_0")).expect("the program that makes the colours");
            let wrong = compile("float4 main() : SV_Target { return nothing; }", s!("ps_4_0"))
                .expect_err("a program that means nothing");
            assert!(wrong.message().contains("nothing"), "{}", wrong.message());
        }
    }

    #[test]
    fn a_card_draws_a_picture_in_its_colours_on_a_pane_and_shows_it() {
        let device = software_card();
        // SAFETY: two windows of a kind Windows has, one inside the other
        // as a pane is, neither shown, both closed below.
        let (around, pane) = unsafe {
            let around = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_POPUP,
                0,
                0,
                200,
                200,
                None,
                None,
                None,
                None,
            )
            .expect("a window");
            let pane = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_CHILD,
                0,
                0,
                64,
                64,
                Some(around),
                None,
                None,
                None,
            )
            .expect("a pane inside it");
            (around, pane)
        };
        let source = picture(&device);
        let laid = (64, 64, DXGI_FORMAT_NV12);
        let mut straight = Straight::new(&device, pane.0 as isize, true).expect("what draws");

        // The 48 rows that show are drawn over the 64 of the pane: red on
        // the left and blue on the right down to the last row, and nothing
        // of the green padding.
        assert!(straight.draw(&device, &source, 0, laid, (64, 48)).expect("a drawing"));
        for y in [0, 31, 63] {
            let (left, right) =
                (pixel(&device, &straight, (8, y)), pixel(&device, &straight, (56, y)));
            assert!(close_to(left, [255, 0, 0]), "row {y}, left: {left:?}");
            assert!(close_to(right, [0, 0, 255]), "row {y}, right: {right:?}");
        }
        // SAFETY: a plain call into the chain, which has its picture.
        unsafe { straight.chain.Present(0, straight.present) }.ok().expect("the picture shown");

        // A pane of another size is drawn on at that size.
        // SAFETY: the pane made above, made smaller.
        unsafe {
            SetWindowPos(pane, None, 0, 0, 32, 20, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE)
                .expect("a smaller pane");
        }
        assert!(straight.draw(&device, &source, 0, laid, (64, 48)).expect("a drawing"));
        assert_eq!(straight.target.as_ref().map(|(_, size)| *size), Some((32, 20)));
        assert!(close_to(pixel(&device, &straight, (4, 19)), [255, 0, 0]));
        assert!(close_to(pixel(&device, &straight, (28, 0)), [0, 0, 255]));
        // SAFETY: as above.
        unsafe { straight.chain.Present(0, straight.present) }.ok().expect("the picture shown");

        // Pictures of another layout than the card's programs read are
        // turned down, and a pane that is gone has nothing drawn on it.
        let other = (64, 64, DXGI_FORMAT_R8G8B8A8_UNORM);
        assert!(straight.draw(&device, &source, 0, other, (64, 48)).is_err());
        // SAFETY: the two windows made above, closed once.
        unsafe {
            DestroyWindow(pane).expect("the pane closed");
            assert!(!straight.draw(&device, &source, 0, laid, (64, 48)).expect("no drawing"));
            DestroyWindow(around).expect("the window closed");
        }
    }

    #[test]
    fn the_program_reads_the_picture_and_none_of_the_padding_around_it() {
        // 1080 rows in a texture of 1088, as a decoder lays them out.
        let told = sizes((3840, 1080), (3840, 1088));
        assert_eq!(told[0], 1.0);
        assert!((told[1] - 1080.0 / 1088.0).abs() < 1e-6);
        // The last row of brightness is read at its middle at most, and
        // the last of colour, two rows high, at its own.
        assert!((told[3] - 1079.5 / 1088.0).abs() < 1e-6);
        assert!((told[5] - 1079.0 / 1088.0).abs() < 1e-6);
        // Colours are read half a pixel to the right, as in `card.rs`.
        assert!((told[6] - 0.5 / 3840.0).abs() < 1e-9);
        assert_eq!(told[7], 0.0);
        assert_eq!(size_of::<Sizes>() % 16, 0, "the card takes sixteen bytes at a time");
    }
}
