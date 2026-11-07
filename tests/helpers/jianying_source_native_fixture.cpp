// SPDX-License-Identifier: GPL-3.0-or-later
// Real compiler RTTI and std::function storage exercise the native reader boundary.
#include <cstring>
#include <functional>
#include <memory>
#include <string>
#ifdef _WIN32
#define EXPORT extern "C" __declspec(dllexport)
constexpr std::size_t source_offset = 0x108, message_offset = 0x148, image_offset = 0x1d0;
#else
#define EXPORT extern "C" __attribute__((visibility("default")))
constexpr std::size_t source_offset = 0xf0, message_offset = 0x128, image_offset = 0x1a8;
#endif
#ifndef FIXTURE_REVISION
#define FIXTURE_REVISION 1
#endif
namespace veplugin { struct FrameData { void* frame; }; struct MessageEvent { bool value; }; }
class TESharedGLContext {};
class TEOfxhImageEffectInstance {
public:
    virtual ~TEOfxhImageEffectInstance() = default;
    unsigned char fields[0x200]{};
};
class TEPluginProcessor {
public:
    unsigned char prefix[source_offset]{};
    std::string source;
    std::function<veplugin::FrameData(const std::string&, double, double, double, double, double)> image;
    std::function<bool(const veplugin::MessageEvent*)> message;
    TEOfxhImageEffectInstance effect;
    void init(const std::shared_ptr<TESharedGLContext>&) {
        auto image_lambda = [this](const std::string&, double, double, double, double, double) { return veplugin::FrameData{this}; };
        image = image_lambda;
        auto message_lambda = [this](const veplugin::MessageEvent* event) { return event && !source.empty(); };
        message = message_lambda;
        auto* image_callable = reinterpret_cast<unsigned char*>(image.target<decltype(image_lambda)>()) - sizeof(void*);
        auto* message_callable = reinterpret_cast<unsigned char*>(message.target<decltype(message_lambda)>()) - sizeof(void*);
        std::memcpy(reinterpret_cast<unsigned char*>(&effect) + image_offset, &image_callable, sizeof(image_callable));
        std::memcpy(reinterpret_cast<unsigned char*>(&effect) + message_offset, &message_callable, sizeof(message_callable));
    }
};
static TEPluginProcessor processor;
EXPORT void* fixture_effect(const char* source) {
    processor.source = source;
    processor.init({});
    return &processor.effect;
}
EXPORT int fixture_revision() { return FIXTURE_REVISION; }
