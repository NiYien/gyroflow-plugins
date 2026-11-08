// SPDX-License-Identifier: GPL-3.0-or-later
#include "metal_guard.h"
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <limits>
#include <string>

static thread_local std::string last_error;
static bool fail(const char* reason) { last_error = reason; return false; }
const char* gf_metal_last_error() { return last_error.c_str(); }

bool gf_metal_validate_buffer(void* resource, void* command_queue, std::size_t required,
                              GFMetalBufferInfo* info) {
    last_error.clear();
    if (!resource || !command_queue) return fail("null Metal resource or queue");
    @autoreleasepool {
        id queue_object = (__bridge id)command_queue;
        if (![queue_object conformsToProtocol:@protocol(MTLCommandQueue)])
            return fail("queue does not conform to MTLCommandQueue");
        id object = (__bridge id)resource;
        if (![object conformsToProtocol:@protocol(MTLBuffer)]
            || [object conformsToProtocol:@protocol(MTLTexture)])
            return fail("resource is not an MTLBuffer");
        id<MTLCommandQueue> queue = queue_object;
        id<MTLBuffer> buffer = object;
        if (!buffer.device || !queue.device || buffer.device != queue.device)
            return fail("buffer and queue devices differ");
        if (buffer.length < required) return fail("buffer is shorter than the image layout");
        if (info) { info->length = buffer.length; info->storage_mode = buffer.storageMode; }
        return true;
    }
}

extern "C" std::int32_t gf_jianying_metal_copy_v1(
    void* source, void* destination, void* command_queue, std::int32_t width, std::int32_t height,
    std::int32_t source_stride, std::int32_t destination_stride, std::int32_t bytes_per_pixel) {
    last_error.clear();
    if (width <= 0 || height <= 0 || (bytes_per_pixel != 4 && bytes_per_pixel != 8 && bytes_per_pixel != 16)
        || source_stride <= 0 || destination_stride <= 0
        || source_stride % bytes_per_pixel != 0 || destination_stride % bytes_per_pixel != 0) {
        fail("copy requires positive RGBA geometry and supported component depth"); return 5;
    }
    const auto row = static_cast<std::uint64_t>(width) * bytes_per_pixel;
    if (row > static_cast<std::uint64_t>(source_stride) || row > static_cast<std::uint64_t>(destination_stride)) {
        fail("copy row exceeds stride"); return 5;
    }
    const auto source_required = static_cast<std::uint64_t>(height - 1) * source_stride + row;
    const auto destination_required = static_cast<std::uint64_t>(height - 1) * destination_stride + row;
    if (source_required > std::numeric_limits<std::size_t>::max()
        || destination_required > std::numeric_limits<std::size_t>::max()) {
        fail("copy layout exceeds addressable size"); return 5;
    }
    GFMetalBufferInfo source_info, destination_info;
    if (!gf_metal_validate_buffer(source, command_queue, source_required, &source_info)
        || !gf_metal_validate_buffer(destination, command_queue, destination_required, &destination_info)) return 5;
    if (source == destination) {
        if (source_stride == destination_stride) return 0;
        fail("overlapping buffer layouts are unsupported"); return 5;
    }
    @autoreleasepool {
        id<MTLCommandQueue> queue = (__bridge id<MTLCommandQueue>)command_queue;
        id<MTLBuffer> input = (__bridge id<MTLBuffer>)source;
        id<MTLBuffer> output = (__bridge id<MTLBuffer>)destination;
        id<MTLCommandBuffer> command = [queue commandBuffer];
        id<MTLBlitCommandEncoder> encoder = [command blitCommandEncoder];
        if (!command || !encoder) { fail("cannot create Metal blit command"); return 5; }
        command.label = @"Gyroflow Jianying GPU passthrough";
        const auto full_size = static_cast<std::uint64_t>(height) * source_stride;
        if (source_stride == destination_stride && full_size <= source_info.length
            && full_size <= destination_info.length) {
            [encoder copyFromBuffer:input sourceOffset:0 toBuffer:output destinationOffset:0 size:full_size];
        } else {
            for (std::int32_t y = 0; y < height; ++y)
                [encoder copyFromBuffer:input sourceOffset:static_cast<NSUInteger>(y) * source_stride
                              toBuffer:output destinationOffset:static_cast<NSUInteger>(y) * destination_stride size:row];
        }
        [encoder endEncoding];
        [command commit];
        [command waitUntilCompleted];
        if (command.status != MTLCommandBufferStatusCompleted) {
            last_error = command.error.localizedDescription.UTF8String ?: "Metal blit did not complete";
            return 1;
        }
        return 0;
    }
}
