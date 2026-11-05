// SPDX-License-Identifier: GPL-3.0-or-later
#include "fixtures/jianying_protocol_host.cpp"
#include <d3d11.h>
#include <wrl/client.h>

using Microsoft::WRL::ComPtr;
static ComPtr<ID3D11Device> gpu_device;
static ComPtr<ID3D11DeviceContext> gpu_context;
static ComPtr<ID3D11Texture2D> gpu_source, gpu_output, gpu_staging;
static std::vector<unsigned char> gpu_readback;
static int gpu_width, gpu_height;

extern "C" __declspec(dllexport) int fixture_gpu_resize(void* ptr, int width, int height, double fps) {
    auto* e = static_cast<Effect*>(ptr);
    if (!gpu_device) {
        D3D_FEATURE_LEVEL feature;
        auto status = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, nullptr, 0,
                                       D3D11_SDK_VERSION, gpu_device.GetAddressOf(), &feature, gpu_context.GetAddressOf());
        if (FAILED(status)) return static_cast<int>(status);
    }
    gpu_source.Reset(); gpu_output.Reset(); gpu_staging.Reset();
    D3D11_TEXTURE2D_DESC description{};
    description.Width = width; description.Height = height; description.MipLevels = 1; description.ArraySize = 1;
    description.Format = DXGI_FORMAT_R8G8B8A8_UNORM; description.SampleDesc.Count = 1;
    description.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
    auto status = gpu_device->CreateTexture2D(&description, nullptr, gpu_source.GetAddressOf());
    if (FAILED(status)) return static_cast<int>(status);
    status = gpu_device->CreateTexture2D(&description, nullptr, gpu_output.GetAddressOf());
    if (FAILED(status)) return static_cast<int>(status);
    description.Usage = D3D11_USAGE_STAGING; description.BindFlags = 0; description.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    status = gpu_device->CreateTexture2D(&description, nullptr, gpu_staging.GetAddressOf());
    if (FAILED(status)) return static_cast<int>(status);
    gpu_width = width; gpu_height = height;
    set_double(handle(e->properties), kOfxImageEffectPropFrameRate, 0, fps);
    for (auto& entry : e->clips) {
        auto& clip = *entry.second;
        clip.pixels.resize(static_cast<size_t>(width) * height * 4);
        auto properties = handle(clip.image_properties);
        const int rectangle[] = {0, 0, width, height};
        const double scale[] = {width / 3840.0, height / 2160.0};
        set_pointer(properties, kOfxImagePropData, 0, clip.pixels.data());
        set_int_n(properties, kOfxImagePropBounds, 4, rectangle);
        set_int_n(properties, kOfxImagePropRegionOfDefinition, 4, rectangle);
        set_int(properties, kOfxImagePropRowBytes, 0, width * 4);
        set_double_n(properties, kOfxImageEffectPropRenderScale, 2, scale);
        set_double(handle(clip.properties), kOfxImageEffectPropFrameRate, 0, fps);
        set_double(handle(clip.properties), kOfxImageEffectPropUnmappedFrameRate, 0, fps);
        set_pointer(properties, "NiYienImagePropD3D11Texture", 0, entry.first == "Source" ? gpu_source.Get() : gpu_output.Get());
        set_pointer(properties, "NiYienImagePropD3D11Device", 0, gpu_device.Get());
        set_pointer(properties, "NiYienImagePropD3D11DeviceContext", 0, gpu_context.Get());
    }
    return 0;
}

extern "C" __declspec(dllexport) int fixture_gpu_frame(void* ptr, unsigned frame) {
    auto* e = static_cast<Effect*>(ptr);
    auto& source = e->clips.at("Source")->pixels;
    auto& output = e->clips.at("Output")->pixels;
    for (int y = 0; y < gpu_height; ++y) for (int x = 0; x < gpu_width; ++x) {
        const auto offset = (static_cast<size_t>(y) * gpu_width + x) * 4;
        source[offset] = static_cast<unsigned char>((x * 13 + y * 3 + frame * 17) % 251);
        source[offset + 1] = static_cast<unsigned char>((x * 5 + y * 11 + frame * 7) % 253);
        source[offset + 2] = static_cast<unsigned char>((x * 7 + y * 19 + frame * 23) % 255);
        source[offset + 3] = 255;
    }
    std::fill(output.begin(), output.end(), static_cast<unsigned char>(0xA7));
    gpu_context->UpdateSubresource(gpu_source.Get(), 0, nullptr, source.data(), gpu_width * 4, 0);
    gpu_context->UpdateSubresource(gpu_output.Get(), 0, nullptr, output.data(), gpu_width * 4, 0);
    gpu_context->Flush();
    return 0;
}

extern "C" __declspec(dllexport) int fixture_gpu_render(void* ptr, double time, int directx) {
    auto* e = static_cast<Effect*>(ptr);
    e->time = time;
    Properties arguments;
    set_double(handle(arguments), kOfxPropTime, 0, time);
    const int rectangle[] = {0, 0, gpu_width, gpu_height};
    const double scale[] = {gpu_width / 3840.0, gpu_height / 2160.0};
    set_int_n(handle(arguments), kOfxImageEffectPropRenderWindow, 4, rectangle);
    set_double_n(handle(arguments), kOfxImageEffectPropRenderScale, 2, scale);
    for (auto name : {"OfxImageEffectPropOpenGLEnabled", "OfxImageEffectPropOpenCLEnabled", "OfxImageEffectPropCudaEnabled", "OfxImageEffectPropMetalEnabled"})
        set_int(handle(arguments), name, 0, 0);
    set_int(handle(arguments), "NiYienImageEffectPropD3D11Enabled", 0, directx);
    return call(e, kOfxImageEffectActionRender, &arguments, nullptr);
}

extern "C" __declspec(dllexport) void fixture_gpu_poison_cpu_source(void* ptr) {
    auto& source = static_cast<Effect*>(ptr)->clips.at("Source")->pixels;
    std::fill(source.begin(), source.end(), static_cast<unsigned char>(0xC3));
}

extern "C" __declspec(dllexport) const unsigned char* fixture_gpu_read(void* ptr, size_t* bytes) {
    gpu_context->CopyResource(gpu_staging.Get(), gpu_output.Get());
    D3D11_MAPPED_SUBRESOURCE mapped{};
    if (FAILED(gpu_context->Map(gpu_staging.Get(), 0, D3D11_MAP_READ, 0, &mapped))) return nullptr;
    gpu_readback.resize(static_cast<size_t>(gpu_width) * gpu_height * 4);
    for (int y = 0; y < gpu_height; ++y)
        std::memcpy(gpu_readback.data() + static_cast<size_t>(y) * gpu_width * 4,
                    static_cast<const unsigned char*>(mapped.pData) + static_cast<size_t>(y) * mapped.RowPitch, gpu_width * 4);
    gpu_context->Unmap(gpu_staging.Get(), 0);
    *bytes = gpu_readback.size();
    return gpu_readback.data();
}
