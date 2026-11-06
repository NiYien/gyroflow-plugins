// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <cstddef>
#include <cstdint>

struct GFMetalBufferInfo {
    std::size_t length = 0;
    unsigned long storage_mode = 0;
};

// Non-null arguments must be live Objective-C objects from the standard OFX Metal contract.
bool gf_metal_validate_buffer(void* resource, void* queue, std::size_t required,
                              GFMetalBufferInfo* info = nullptr);
const char* gf_metal_last_error();
extern "C" std::int32_t gf_jianying_metal_copy_v1(
    void* source, void* destination, void* queue, std::int32_t width, std::int32_t height,
    std::int32_t source_stride, std::int32_t destination_stride, std::int32_t bytes_per_pixel);
