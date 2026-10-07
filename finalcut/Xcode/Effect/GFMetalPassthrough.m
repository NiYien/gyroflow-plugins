// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFMetalPassthrough.h"
#import <math.h>
#import <simd/simd.h>

static NSError *GFPassthroughError(NSString *message) {
    return [NSError errorWithDomain:@"com.niyien.gyroflow.finalcut.passthrough"
                               code:1 userInfo:@{NSLocalizedDescriptionKey: message}];
}

typedef struct GFPassthroughMapping {
    vector_float4 sourceBounds;
    vector_float4 destinationBounds;
    uint32_t flipY;
} GFPassthroughMapping;

static BOOL GFContentBounds(GFHostImageV2 image, id<MTLTexture> texture,
                            vector_float4 *bounds) {
    if ((image.origin != 0 && image.origin != 2) || image.reserved != 0 ||
        image.texture.width != texture.width || image.texture.height != texture.height) {
        return NO;
    }
    for (NSUInteger i = 0; i < 4; i++) {
        if (!isfinite(image.image_rect[i]) || !isfinite(image.tile_rect[i])) { return NO; }
    }
    double width = image.tile_rect[2] - image.tile_rect[0];
    double height = image.tile_rect[3] - image.tile_rect[1];
    if (fabs(width - texture.width) > 1e-4 || fabs(height - texture.height) > 1e-4) {
        return NO;
    }
    double left = image.image_rect[0] - image.tile_rect[0];
    double bottom = image.image_rect[1] - image.tile_rect[1];
    double right = image.image_rect[2] - image.tile_rect[0];
    double top = image.image_rect[3] - image.tile_rect[1];
    if (right - left < 1 || top - bottom < 1 || left < 0 || bottom < 0 ||
        right > width || top > height) { return NO; }
    *bounds = (vector_float4){left, bottom, right, top};
    return YES;
}

@interface GFMetalPassthrough ()
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, id<MTLRenderPipelineState>> *pipelines;
@end

@implementation GFMetalPassthrough
- (instancetype)initWithDevice:(id<MTLDevice>)device {
    self = [super init];
    if (self != nil) {
        self.device = device;
        self.pipelines = [NSMutableDictionary dictionary];
    }
    return self;
}

- (id<MTLRenderPipelineState>)pipelineForFormat:(MTLPixelFormat)format error:(NSError **)error {
    @synchronized(self) {
        NSNumber *key = @(format);
        id<MTLRenderPipelineState> cached = self.pipelines[key];
        if (cached != nil) { return cached; }
        NSString *shader = @"#include <metal_stdlib>\n"
            "using namespace metal;\n"
            "struct Mapping { float4 sourceBounds; float4 destinationBounds; uint flipY; };\n"
            "vertex float4 fullscreen(uint v [[vertex_id]]) {\n"
            " const float2 p[] = {float2(-1,-1), float2(3,-1), float2(-1,3)};\n"
            " return float4(p[v],0,1); }\n"
            "fragment half4 passthrough(float4 p [[position]],\n"
            " texture2d<half> input [[texture(0)]], constant Mapping &m [[buffer(0)]]) {\n"
            " if (any(p.xy < m.destinationBounds.xy) || any(p.xy >= m.destinationBounds.zw))\n"
            "   return half4(0);\n"
            " float2 uv = (p.xy-m.destinationBounds.xy)/(m.destinationBounds.zw-m.destinationBounds.xy);\n"
            " if (m.flipY) uv.y = 1-uv.y;\n"
            " float2 q = m.sourceBounds.xy+uv*(m.sourceBounds.zw-m.sourceBounds.xy);\n"
            " q = clamp(q, m.sourceBounds.xy+0.5, m.sourceBounds.zw-0.5);\n"
            " constexpr sampler s(coord::pixel, address::clamp_to_edge, filter::linear);\n"
            " return input.sample(s,q); }\n";
        id<MTLLibrary> library = [self.device newLibraryWithSource:shader options:nil error:error];
        if (library == nil) { return nil; }
        MTLRenderPipelineDescriptor *descriptor = [[MTLRenderPipelineDescriptor alloc] init];
        descriptor.vertexFunction = [library newFunctionWithName:@"fullscreen"];
        descriptor.fragmentFunction = [library newFunctionWithName:@"passthrough"];
        descriptor.colorAttachments[0].pixelFormat = format;
        cached = [self.device newRenderPipelineStateWithDescriptor:descriptor error:error];
        if (cached != nil) { self.pipelines[key] = cached; }
        return cached;
    }
}

- (BOOL)copySource:(id<MTLTexture>)source sourceImage:(GFHostImageV2)sourceImage
      destination:(id<MTLTexture>)destination destinationImage:(GFHostImageV2)destinationImage
     commandQueue:(id<MTLCommandQueue>)queue gpuSeconds:(double *)gpuSeconds error:(NSError **)error {
    if (gpuSeconds != NULL) { *gpuSeconds = 0; }
    GFPassthroughMapping mapping = {0};
    if (source == nil || destination == nil || queue == nil ||
        source.device.registryID != self.device.registryID ||
        destination.device.registryID != self.device.registryID ||
        queue.device.registryID != self.device.registryID ||
        source.pixelFormat != destination.pixelFormat ||
        !GFContentBounds(sourceImage, source, &mapping.sourceBounds) ||
        !GFContentBounds(destinationImage, destination, &mapping.destinationBounds)) {
        if (error != NULL) { *error = GFPassthroughError(@"Invalid Metal passthrough image metadata"); }
        return NO;
    }
    mapping.flipY = sourceImage.origin != destinationImage.origin;
    id<MTLCommandBuffer> command = [queue commandBuffer];
    if (command == nil) {
        if (error != NULL) { *error = GFPassthroughError(@"Unable to create Metal passthrough command"); }
        return NO;
    }
    id<MTLRenderPipelineState> pipeline = [self pipelineForFormat:destination.pixelFormat error:error];
    if (pipeline == nil) { return NO; }
    MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
    pass.colorAttachments[0].texture = destination;
    pass.colorAttachments[0].loadAction = MTLLoadActionDontCare;
    pass.colorAttachments[0].storeAction = MTLStoreActionStore;
    id<MTLRenderCommandEncoder> encoder = [command renderCommandEncoderWithDescriptor:pass];
    if (encoder == nil) {
        if (error != NULL) { *error = GFPassthroughError(@"Unable to encode Metal passthrough render"); }
        return NO;
    }
    // FxPlug output textures support render targets, without requiring shader writes.
    [encoder setRenderPipelineState:pipeline];
    [encoder setFragmentTexture:source atIndex:0];
    [encoder setFragmentBytes:&mapping length:sizeof(mapping) atIndex:0];
    [encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
    [encoder endEncoding];

    [command commit];
    [command waitUntilCompleted];
    if (gpuSeconds != NULL && command.GPUStartTime > 0 && command.GPUEndTime >= command.GPUStartTime) {
        *gpuSeconds = command.GPUEndTime - command.GPUStartTime;
    }
    if (command.status == MTLCommandBufferStatusError) {
        if (error != NULL) { *error = command.error ?: GFPassthroughError(@"Metal passthrough failed"); }
        return NO;
    }
    return YES;
}
@end
