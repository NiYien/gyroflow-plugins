// SPDX-License-Identifier: GPL-3.0-or-later
// Real compiler RTTI and std::function storage exercise the native reader boundary.
#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <string>
#ifndef FIXTURE_REVISION
#define FIXTURE_REVISION 1
#endif
namespace veplugin { struct FrameData { void* frame; }; struct MessageEvent { bool value; }; }
class TESharedGLContext {};

#ifdef _WIN32
#define EXPORT extern "C" __declspec(dllexport)
#define NOINLINE __declspec(noinline)

struct FrameRequest {
    std::string source;
    std::string clip_id;
    std::string clip_name;
};

static volatile std::size_t request_sink = 0;

static NOINLINE void consume_request(const FrameRequest& request) {
    request_sink = request.source.size() + request.clip_id.size() + request.clip_name.size();
}

class TEOfxhImageEffectInstance {
public:
    virtual ~TEOfxhImageEffectInstance() = default;
    virtual void fixture_action() {}
#if FIXTURE_REVISION == 1
    unsigned char pad0[0x108]{};
    std::function<bool(const veplugin::MessageEvent*)> message;
    unsigned char pad1[0x48]{};
    std::function<veplugin::FrameData(const std::string&, double, double, double, double, double)> image;
#elif FIXTURE_REVISION == 2
    unsigned char pad0[0x148]{};
    std::function<bool(const veplugin::MessageEvent*)> message;
    unsigned char pad1[0x90]{};
    std::function<veplugin::FrameData(const std::string&, double, double, double, double, double)> image;
#else
    unsigned char pad0[0x118]{};
    std::function<veplugin::FrameData(const std::string&, double, double, double, double, double)> image;
    unsigned char pad1[0x60]{};
    std::function<bool(const veplugin::MessageEvent*)> message;
#endif
};

class TEPluginProcessor : public std::enable_shared_from_this<TEPluginProcessor> {
public:
#if FIXTURE_REVISION == 1
    unsigned char pad0[0x48]{};
    std::string clip_id;
    unsigned char pad1[0x90]{};
    std::string source;
#elif FIXTURE_REVISION == 2
    unsigned char pad0[0x58]{};
    std::string clip_id;
    unsigned char pad1[0x98]{};
    std::string source;
#else
    unsigned char pad0[0x28]{};
    std::string clip_id;
    unsigned char pad1[0x138]{};
    std::string source;
#endif
    TEOfxhImageEffectInstance effect;

    NOINLINE veplugin::FrameData fetchFrameFromEditor(const std::string& clip, std::int64_t time) {
        (void)time;
        if (clip.size() == 6 && std::memcmp(clip.data(), "Source", 6) == 0) {
            FrameRequest request;
            request.source = source;
            request.clip_id = clip_id;
            request.clip_name = clip;
            consume_request(request);
            return veplugin::FrameData{this};
        }
        return veplugin::FrameData{nullptr};
    }

    NOINLINE veplugin::FrameData getClipFrame(const std::string& clip, double time) {
        return fetchFrameFromEditor(clip, static_cast<std::int64_t>(time));
    }

    void init(const std::shared_ptr<TESharedGLContext>&, bool install_message) {
#if FIXTURE_REVISION == 1
        effect.image = [this](const std::string& clip, double time, double, double, double, double) {
            return fetchFrameFromEditor(clip, static_cast<std::int64_t>(time));
        };
        if (install_message) {
            effect.message = [this](const veplugin::MessageEvent* event) { return event && !source.empty(); };
        } else {
            effect.message = nullptr;
        }
#elif FIXTURE_REVISION == 2
        auto weak_self = weak_from_this();
        effect.image = [weak_self](const std::string& clip, double time, double, double, double, double) {
            if (auto locked = weak_self.lock()) return locked->getClipFrame(clip, time);
            return veplugin::FrameData{nullptr};
        };
        if (install_message) {
            bool use_weak = true;
            effect.message = [this, weak_self, use_weak](const veplugin::MessageEvent* event) {
                auto locked = use_weak ? weak_self.lock() : nullptr;
                const auto* target = use_weak ? locked.get() : this;
                return target && event && !target->source.empty();
            };
        } else {
            effect.message = nullptr;
        }
#else
        effect.image = [this](const std::string& clip, double time, double, double, double, double) {
            return getClipFrame(clip, time);
        };
        if (install_message) {
            effect.message = [this](const veplugin::MessageEvent* event) { return event && !source.empty(); };
        } else {
            effect.message = nullptr;
        }
#endif
    }
};

static std::shared_ptr<TEPluginProcessor> processor;

EXPORT void* fixture_effect(const char* source) {
    processor = std::make_shared<TEPluginProcessor>();
    processor->clip_id = "seg#clip";
    processor->source = source;
    processor->init({}, true);
    return &processor->effect;
}

EXPORT void* fixture_pending_effect(const char* source) {
    processor = std::make_shared<TEPluginProcessor>();
    processor->clip_id = "seg#clip";
    processor->source = source;
    processor->init({}, false);
    return &processor->effect;
}

EXPORT void fixture_attach_message() {
    if (processor) processor->init({}, true);
}

EXPORT int fixture_revision() { return FIXTURE_REVISION; }

#else
#define EXPORT extern "C" __attribute__((visibility("default")))
#if FIXTURE_REVISION == 1
#define FIXTURE_SOURCE 0xc0
#define FIXTURE_MESSAGE 0x130
#define FIXTURE_IMAGE 0x1b0
#else
#define FIXTURE_SOURCE 0x190
#define FIXTURE_MESSAGE 0x150
#define FIXTURE_IMAGE 0x1e0
#endif
constexpr std::size_t source_offset = FIXTURE_SOURCE, message_offset = FIXTURE_MESSAGE, image_offset = FIXTURE_IMAGE;
class TEOfxhImageEffectInstance {
public:
    virtual ~TEOfxhImageEffectInstance() = default;
    virtual void fixture_action() {}
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

#if defined(__APPLE__) && defined(__aarch64__)
#define STR_INNER(n) #n
#define STR(n) STR_INNER(n)
// The host-shaped instruction sequences are only read as data. Native RTTI
// and std::function storage above provide the actual path-bearing objects.
asm(
    ".text\n.p2align 2\n"
    ".globl __ZN17TEPluginProcessor20fetchFrameFromEditorERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEEx\n"
    "__ZN17TEPluginProcessor20fetchFrameFromEditorERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEEx:\n"
    "mov x20, x0\n"
    "mov w10, #0x6f53\n"
    "movk w10, #0x7275, lsl #16\n"
    "cmp w9, w10\n"
    "mov w9, #0x6563\n"
    "ccmp w8, w9, #0, eq\n"
    "b.ne 1f\n"
    "add x23, sp, #32\n"
    "add x0, sp, #32\n"
    "add x1, x20, #" STR(FIXTURE_SOURCE) "\n"
    "bl 1f\n"
    "add x0, x23, #24\n"
    "add x1, x20, #88\n"
    "bl 1f\n"
    "add x0, x23, #48\n"
    "mov x1, x22\n"
    "bl 1f\n"
    "1: ret\n"
    ".fill 96, 4, 0\n"
    ".globl __ZN25TEOfxhImageEffectInstance8populateENSt3__18functionIFN8veplugin9FrameDataERKNS0_12basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEEdddddEEE\n"
    "__ZN25TEOfxhImageEffectInstance8populateENSt3__18functionIFN8veplugin9FrameDataERKNS0_12basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEEdddddEEE:\n"
    "mov x19, x0\n"
    "add x20, x0, #(" STR(FIXTURE_IMAGE) " - 24)\n"
    "ldr x0, [x0, #" STR(FIXTURE_IMAGE) "]\n"
    "str xzr, [x19, #" STR(FIXTURE_IMAGE) "]\n"
    "cmp x0, x20\n"
    "b.eq 2f\n"
    "2: ret\n"
    ".fill 96, 4, 0\n"
    ".globl __ZN19TEOfxPluginInstance18setMessageCallbackERKNSt3__18functionIFbPKN8veplugin12MessageEventEEEE\n"
    "__ZN19TEOfxPluginInstance18setMessageCallbackERKNSt3__18functionIFbPKN8veplugin12MessageEventEEEE:\n"
    "ldr x20, [x0, #8]\n"
    "ldr x0, [x1, #24]\n"
    "cbz x0, 3f\n"
    "add x19, x20, #(" STR(FIXTURE_MESSAGE) " - 24)\n"
    "nop\n"
    "nop\n"
    "nop\n"
    "ldr x0, [sp, #24]\n"
    "ldr x8, [x20, #" STR(FIXTURE_MESSAGE) "]\n"
    "nop\n"
    "nop\n"
    "cmp x8, x19\n"
    "nop\n"
    "str x8, [sp, #24]\n"
    "str x0, [x20, #" STR(FIXTURE_MESSAGE) "]\n"
    "3: ret\n"
    ".fill 96, 4, 0\n");
#endif
#endif
