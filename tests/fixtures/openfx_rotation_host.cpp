// SPDX-License-Identifier: GPL-3.0-or-later
#include "jianying_protocol_host.cpp"

extern "C" __declspec(dllexport) void fixture_rotation_images(void* ptr, int source_width, int source_height,
    int output_width, int output_height, double scale, const unsigned char* input) {
    auto* e = static_cast<Effect*>(ptr);
    const double project_size[] = {output_width / scale, output_height / scale};
    set_double_n(handle(e->properties), kOfxImageEffectPropProjectSize, 2, project_size);
    for (auto& entry : e->clips) {
        auto& c = *entry.second;
        const int width = c.name == "Source" ? source_width : output_width;
        const int height = c.name == "Source" ? source_height : output_height;
        c.pixels.resize(width * height * 4);
        if (c.name == "Source") std::copy(input, input + c.pixels.size(), c.pixels.begin());
        c.rod[2] = width / scale;
        c.rod[3] = height / scale;
        auto p = handle(c.image_properties);
        set_pointer(p, kOfxImagePropData, 0, c.pixels.data());
        const int rect[] = {0, 0, width, height};
        set_int_n(p, kOfxImagePropBounds, 4, rect);
        set_int_n(p, kOfxImagePropRegionOfDefinition, 4, rect);
        set_int(p, kOfxImagePropRowBytes, 0, width * 4);
        // The real ANGLE path bypasses the legacy CPU-image render-scale adjustment.
        const double image_scale[] = {1, 1};
        set_double_n(p, kOfxImageEffectPropRenderScale, 2, image_scale);
    }
}

extern "C" __declspec(dllexport) int fixture_rotation_render(void* ptr, double time, double scale) {
    auto* e = static_cast<Effect*>(ptr);
    e->time = time;
    Properties in, out;
    set_double(handle(in), kOfxPropTime, 0, time);
    const double render_scale[] = {scale, scale};
    set_double_n(handle(in), kOfxImageEffectPropRenderScale, 2, render_scale);
    auto& image = e->clips.at("Output")->image_properties;
    int width = 0, height = 0;
    get_int(handle(image), kOfxImagePropBounds, 2, &width);
    get_int(handle(image), kOfxImagePropBounds, 3, &height);
    const int rect[] = {0, 0, width, height};
    set_int_n(handle(in), kOfxImageEffectPropRenderWindow, 4, rect);
    for (auto name : {"OfxImageEffectPropOpenGLEnabled", "OfxImageEffectPropOpenCLEnabled", "OfxImageEffectPropCudaEnabled", "OfxImageEffectPropMetalEnabled"}) set_int(handle(in), name, 0, 0);
    std::fill(e->clips.at("Output")->pixels.begin(), e->clips.at("Output")->pixels.end(), 173);
    return call(e, kOfxImageEffectActionRender, &in, &out);
}
