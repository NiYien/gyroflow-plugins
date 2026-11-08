// SPDX-License-Identifier: GPL-3.0-or-later
// Pixel reads are confined to these offline fixtures, never the host adapter.
#include "metal_guard.h"
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <algorithm>
#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <vector>

static void require(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
static void copy_for_test(id<MTLCommandQueue> queue, id<MTLBuffer> source, id<MTLBuffer> output, NSUInteger size) {
    id<MTLCommandBuffer> command = [queue commandBuffer];
    id<MTLBlitCommandEncoder> encoder = [command blitCommandEncoder];
    [encoder copyFromBuffer:source sourceOffset:0 toBuffer:output destinationOffset:0 size:size];
    [encoder endEncoding]; [command commit]; [command waitUntilCompleted];
    require(command.status == MTLCommandBufferStatusCompleted, "test transfer failed");
}
static void fixture(id<MTLDevice> device, id<MTLCommandQueue> queue, int width, int height,
                    int input_stride, int output_stride, bool private_buffers, int bytes_per_pixel = 4) {
    const auto input_size = static_cast<std::size_t>(input_stride) * height;
    const auto output_size = static_cast<std::size_t>(output_stride) * height;
    std::vector<unsigned char> input(input_size), expected(output_size, 0xcc);
    for (std::size_t i = 0; i < input.size(); ++i) input[i] = static_cast<unsigned char>((i * 29 + i / input_stride * 7) % 251);
    id<MTLBuffer> upload = [device newBufferWithBytes:input.data() length:input_size options:MTLResourceStorageModeShared];
    id<MTLBuffer> source = private_buffers ? [device newBufferWithLength:input_size options:MTLResourceStorageModePrivate] : upload;
    id<MTLBuffer> output = [device newBufferWithLength:output_size options:private_buffers ? MTLResourceStorageModePrivate : MTLResourceStorageModeShared];
    require(source && output, "fixture allocation failed");
    if (private_buffers) copy_for_test(queue, upload, source, input_size);
    id<MTLCommandBuffer> clear = [queue commandBuffer];
    id<MTLBlitCommandEncoder> encoder = [clear blitCommandEncoder];
    [encoder fillBuffer:output range:NSMakeRange(0, output_size) value:0xcc];
    [encoder endEncoding]; [clear commit]; [clear waitUntilCompleted];
    require(clear.status == MTLCommandBufferStatusCompleted, "fixture clear failed");
    require(gf_jianying_metal_copy_v1((__bridge void*)source, (__bridge void*)output, (__bridge void*)queue,
                                     width, height, input_stride, output_stride, bytes_per_pixel) == 0, gf_metal_last_error());
    id<MTLBuffer> readback = [device newBufferWithLength:output_size options:MTLResourceStorageModeShared];
    copy_for_test(queue, output, readback, output_size);
    if (input_stride == output_stride) expected = input;
    else for (int y = 0; y < height; ++y)
        std::memcpy(expected.data() + static_cast<std::size_t>(y) * output_stride,
                    input.data() + static_cast<std::size_t>(y) * input_stride, static_cast<std::size_t>(width) * bytes_per_pixel);
    require(std::memcmp(readback.contents, expected.data(), output_size) == 0, "GPU passthrough pixels or padding differ");
    require(gf_jianying_metal_copy_v1((__bridge void*)source, (__bridge void*)source, (__bridge void*)queue,
                                     width, height, input_stride, input_stride, bytes_per_pixel) == 0, "identical buffer should be a no-op");
}

int main() {
    @autoreleasepool {
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (!device) { std::fprintf(stderr, "SKIP: no Metal device in this permission context\n"); return 77; }
        try {
            id<MTLCommandQueue> queue = [device newCommandQueue];
            require(queue != nil, "no Metal command queue");
            fixture(device, queue, 640, 360, 2560, 2560, false);
            fixture(device, queue, 7, 5, 32, 48, false);
            fixture(device, queue, 7, 5, 32, 48, true);
            fixture(device, queue, 3840, 2160, 15360, 15360, true);
            for (int bytes_per_pixel : {8, 16}) {
                fixture(device, queue, 7, 5, 8 * bytes_per_pixel, 10 * bytes_per_pixel, false, bytes_per_pixel);
                fixture(device, queue, 7, 5, 8 * bytes_per_pixel, 10 * bytes_per_pixel, true, bytes_per_pixel);
                fixture(device, queue, 3840, 2160, 3840 * bytes_per_pixel, 3840 * bytes_per_pixel, true, bytes_per_pixel);
            }
            id<MTLBuffer> small = [device newBufferWithLength:16 options:MTLResourceStorageModeShared];
            require(!gf_metal_validate_buffer((__bridge void*)small, (__bridge void*)queue, 20), "short buffer accepted");
            require(gf_jianying_metal_copy_v1((__bridge void*)small, (__bridge void*)small, (__bridge void*)queue,
                                             2, 2, 8, 8, 2) == 5, "unsupported RGBA pixel width accepted");
            require(gf_jianying_metal_copy_v1((__bridge void*)small, (__bridge void*)small, (__bridge void*)queue,
                                             2, 2, 16, 16, 8) == 5, "short high-depth buffer accepted");
            require(gf_jianying_metal_copy_v1((__bridge void*)small, (__bridge void*)small, (__bridge void*)queue,
                                             2, 2, 9, 8, 4) == 5, "unaligned stride accepted");
            require(gf_jianying_metal_copy_v1((__bridge void*)small, (__bridge void*)small, (__bridge void*)queue,
                                             -1, 2, 8, 8, 4) == 5, "negative width accepted");
            require(gf_jianying_metal_copy_v1(nullptr, (__bridge void*)small, (__bridge void*)queue,
                                             2, 2, 8, 8, 4) == 5, "null source accepted");
            require(!gf_metal_validate_buffer((__bridge void*)small, (__bridge void*)device, 4), "non-queue object accepted");
            auto descriptor = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm
                                                                               width:2 height:2 mipmapped:NO];
            id<MTLTexture> texture = [device newTextureWithDescriptor:descriptor];
            require(texture && !gf_metal_validate_buffer((__bridge void*)texture, (__bridge void*)queue, 4), "texture accepted as buffer");
            std::printf("PASS: real Metal GPU blit RGBA8/16/float, contiguous 640x360/4K, unequal strides, shared/private, padding and rejection checks\n");
            return 0;
        } catch (const std::exception& error) {
            std::fprintf(stderr, "FAIL: %s\n", error.what()); return 1;
        }
    }
}
