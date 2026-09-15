// SPDX-License-Identifier: GPL-3.0-or-later
// Host image origins are normalized inside FCP with an exact texel permutation.
// The shared stabilizer, its shader layout and interpolation remain unchanged.
use crate::{GFDimensionsU32, GFMetalRenderRequestV2};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::*;
use std::ffi::c_void;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Queue = Retained<ProtocolObject<dyn MTLCommandQueue>>;

fn normalize_top_left(image: &mut crate::host_geometry::GFHostImageV2) {
    // Reflect asymmetric content bounds with the rows, preserving the tile extent.
    let sum = image.tile_rect[1] + image.tile_rect[3];
    let bounds = image.image_rect;
    image.image_rect[1] = sum - bounds[3];
    image.image_rect[3] = sum - bounds[1];
    image.origin = 2;
}

#[derive(Default)]
pub struct MetalImages {
    device_id: u64,
    pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
    source: Option<Texture>,
    destination: Option<Texture>,
}
// All access is serialized by the instance mutex. Metal resources may be retained
// and used on different render threads; no AppKit objects are stored here.
unsafe impl Send for MetalImages {}

pub struct FrameImages {
    pub request: GFMetalRenderRequestV2,
    queue: Queue,
    source: Texture,
    destination: Texture,
    output: Texture,
}
impl MetalImages {
    fn temporary(
        slot: &mut Option<Texture>,
        device: &ProtocolObject<dyn MTLDevice>,
        size: GFDimensionsU32,
    ) -> Result<Texture, String> {
        if let Some(texture) = slot {
            if texture.width() == size.width as usize && texture.height() == size.height as usize {
                return Ok(texture.clone());
            }
        }
        let d = MTLTextureDescriptor::new();
        unsafe {
            d.setWidth(size.width as usize);
            d.setHeight(size.height as usize);
        }
        d.setPixelFormat(MTLPixelFormat::RGBA16Float);
        d.setStorageMode(MTLStorageMode::Private);
        d.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
        let texture = device
            .newTextureWithDescriptor(&d)
            .ok_or("Unable to allocate FCP orientation texture")?;
        *slot = Some(texture.clone());
        Ok(texture)
    }
    fn flip(
        &mut self,
        queue: &ProtocolObject<dyn MTLCommandQueue>,
        source: &ProtocolObject<dyn MTLTexture>,
        destination: &ProtocolObject<dyn MTLTexture>,
    ) -> Result<(), String> {
        if self.pipeline.is_none() {
            let shader = NSString::from_str(
                r#"
                #include <metal_stdlib>
                using namespace metal;
                vertex float4 fullscreen(uint v [[vertex_id]]) {
                    const float2 points[] = {float2(-1,-1), float2(3,-1), float2(-1,3)};
                    return float4(points[v], 0, 1);
                }
                fragment half4 flip_rows(float4 position [[position]],
                                         texture2d<half, access::read> input [[texture(0)]]) {
                    uint2 p = uint2(position.xy);
                    return input.read(uint2(p.x, input.get_height() - 1 - p.y));
                }
            "#,
            );
            let device = queue.device();
            let library = device
                .newLibraryWithSource_options_error(&shader, None)
                .map_err(|e| e.to_string())?;
            let function = library
                .newFunctionWithName(&NSString::from_str("flip_rows"))
                .ok_or("Missing FCP orientation kernel")?;
            let vertex = library
                .newFunctionWithName(&NSString::from_str("fullscreen"))
                .ok_or("Missing FCP orientation vertex function")?;
            let descriptor = MTLRenderPipelineDescriptor::new();
            descriptor.setVertexFunction(Some(&vertex));
            descriptor.setFragmentFunction(Some(&function));
            unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }
                .setPixelFormat(MTLPixelFormat::RGBA16Float);
            self.pipeline = Some(
                device
                    .newRenderPipelineStateWithDescriptor_error(&descriptor)
                    .map_err(|e| e.to_string())?,
            );
        }
        let command = queue
            .commandBuffer()
            .ok_or("Unable to create FCP orientation command")?;
        // Match the existing core's ShaderRead/RenderTarget requirements.
        // Host output textures need not allow compute shader writes.
        let pass = MTLRenderPassDescriptor::new();
        let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(destination));
        attachment.setLoadAction(MTLLoadAction::DontCare);
        attachment.setStoreAction(MTLStoreAction::Store);
        let encoder = command
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or("Unable to encode FCP orientation command")?;
        encoder.setRenderPipelineState(self.pipeline.as_ref().unwrap());
        unsafe {
            encoder.setFragmentTexture_atIndex(Some(source), 0);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        encoder.endEncoding();
        command.commit();
        command.waitUntilCompleted();
        if command.status() == MTLCommandBufferStatus::Error {
            return Err("FCP orientation command failed".into());
        }
        Ok(())
    }
    pub unsafe fn begin(&mut self, request: GFMetalRenderRequestV2) -> Result<FrameImages, String> {
        // The C bridge requires live host-owned Metal objects for the whole call.
        let queue = unsafe {
            Retained::<ProtocolObject<dyn MTLCommandQueue>>::retain(request.command_queue.cast())
        }
        .ok_or("Missing Metal queue")?;
        let source = unsafe {
            Retained::<ProtocolObject<dyn MTLTexture>>::retain(request.input_texture.cast())
        }
        .ok_or("Missing Metal input")?;
        let output = unsafe {
            Retained::<ProtocolObject<dyn MTLTexture>>::retain(request.output_texture.cast())
        }
        .ok_or("Missing Metal output")?;
        let device = queue.device();
        if device.registryID() != request.device_registry_id {
            return Err("Host Metal queue does not match the render device".into());
        }
        for (texture, size) in [
            (&source, request.source.texture),
            (&output, request.destination.texture),
        ] {
            if texture.width() != size.width as usize
                || texture.height() != size.height as usize
                || texture.pixelFormat() != MTLPixelFormat::RGBA16Float
                || texture.device().registryID() != device.registryID()
            {
                return Err("Host Metal texture does not match its metadata".into());
            }
        }
        if self.device_id != request.device_registry_id {
            *self = Self {
                device_id: request.device_registry_id,
                ..Default::default()
            };
        }
        let mut normalized = request;
        let source = if request.source.origin == 0 {
            let texture = Self::temporary(&mut self.source, &device, request.source.texture)?;
            self.flip(&queue, &source, &texture)?;
            normalized.input_texture = Retained::as_ptr(&texture) as *mut c_void;
            normalize_top_left(&mut normalized.source);
            texture
        } else {
            source
        };
        let destination = if request.destination.origin == 0 {
            let texture =
                Self::temporary(&mut self.destination, &device, request.destination.texture)?;
            normalized.output_texture = Retained::as_ptr(&texture) as *mut c_void;
            normalize_top_left(&mut normalized.destination);
            texture
        } else {
            output.clone()
        };
        Ok(FrameImages {
            request: normalized,
            queue,
            source,
            destination,
            output,
        })
    }
    pub fn finish(&mut self, frame: &FrameImages) -> Result<(), String> {
        if Retained::as_ptr(&frame.destination) != Retained::as_ptr(&frame.output) {
            self.flip(&frame.queue, &frame.destination, &frame.output)?;
        }
        // Retain the source until the stabilizer and any final copy have completed.
        let _ = &frame.source;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_down_normalization_reflects_asymmetric_content_bounds_with_rows() {
        let mut image = crate::host_geometry::GFHostImageV2 {
            image_rect: [12.0, 24.0, 76.0, 72.0],
            tile_rect: [10.0, 20.0, 80.0, 74.0],
            origin: 0,
            ..Default::default()
        };
        normalize_top_left(&mut image);
        assert_eq!(image.origin, 2);
        assert_eq!(image.image_rect, [12.0, 22.0, 76.0, 70.0]);
        assert_eq!(image.tile_rect, [10.0, 20.0, 80.0, 74.0]);
    }
}
