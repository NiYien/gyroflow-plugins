// SPDX-License-Identifier: GPL-3.0-or-later
// A deliberately small OFX implementation used only by the native adapter ABI test.
#include <ofxCore.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#include <cstdint>
#include <cstring>

namespace {
OfxHost* host = nullptr;
const OfxPropertySuiteV1* properties = nullptr;
const OfxImageEffectSuiteV1* images = nullptr;
const OfxParameterSuiteV1* parameters = nullptr;
unsigned render_calls = 0;
unsigned action_calls = 0;
using Copy = std::int32_t (*)(void*, void*, void*, std::int32_t, std::int32_t,
                            std::int32_t, std::int32_t, std::int32_t);
Copy copy = nullptr;
void set_host(OfxHost* value) {
    host = value;
    properties = static_cast<const OfxPropertySuiteV1*>(host->fetchSuite(host->host, kOfxPropertySuite, 1));
    images = static_cast<const OfxImageEffectSuiteV1*>(host->fetchSuite(host->host, kOfxImageEffectSuite, 1));
    parameters = static_cast<const OfxParameterSuiteV1*>(host->fetchSuite(host->host, kOfxParameterSuite, 1));
}
OfxStatus entry(const char* action, const void* raw_effect, OfxPropertySetHandle in, OfxPropertySetHandle) {
    ++action_calls;
    if (!host || !properties || !images || !parameters) return kOfxStatErrMissingHostFeature;
    auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(raw_effect));
    if (!std::strcmp(action, "MockUnknownAction")) return kOfxStatReplyYes;
    if (!std::strcmp(action, kOfxActionLoad)) {
        int adapter = 0, inverted = -1, count = 0;
        void* callback = nullptr;
        if (properties->propGetInt(host->host, "NiYienJianyingMacMetalAdapter", 0, &adapter) != kOfxStatOK || adapter != 1
            || properties->propGetIntN(host->host, "NiYienJianyingMacFramebufferInverted", 1, &inverted) != kOfxStatOK || inverted != 0
            || properties->propGetDimension(host->host, "NiYienJianyingMacMetalCopyV1", &count) != kOfxStatOK || count != 1
            || properties->propGetPointerN(host->host, "NiYienJianyingMacMetalCopyV1", 1, &callback) != kOfxStatOK || !callback)
            return kOfxStatFailed;
        copy = reinterpret_cast<Copy>(callback);
        const int integer = 0;
        const double real = 0;
        const char* text = "forbidden";
        void* pointer = nullptr;
        if (properties->propSetInt(host->host, "NiYienJianyingMacMetalAdapter", 0, 0) != kOfxStatErrUnsupported
            || properties->propSetPointer(host->host, "NiYienJianyingMacMetalCopyV1", 0, nullptr) != kOfxStatErrUnsupported
            || properties->propSetString(host->host, "NiYienJianyingMacMetalAdapter", 0, text) != kOfxStatErrUnsupported
            || properties->propSetDouble(host->host, "NiYienJianyingMacMetalAdapter", 0, real) != kOfxStatErrUnsupported
            || properties->propSetIntN(host->host, "NiYienJianyingMacMetalAdapter", 1, &integer) != kOfxStatErrUnsupported
            || properties->propSetPointerN(host->host, "NiYienJianyingMacMetalCopyV1", 1, &pointer) != kOfxStatErrUnsupported
            || properties->propSetStringN(host->host, "NiYienJianyingMacMetalAdapter", 1, &text) != kOfxStatErrUnsupported
            || properties->propSetDoubleN(host->host, "NiYienJianyingMacMetalAdapter", 1, &real) != kOfxStatErrUnsupported
            || properties->propReset(host->host, "NiYienJianyingMacMetalAdapter") != kOfxStatErrUnsupported)
            return kOfxStatFailed;
        return kOfxStatOK;
    }
    if (!std::strcmp(action, "MockVarargs")) {
        OfxParamSetHandle set = nullptr;
        images->getParamSet(effect, &set);
        OfxParamHandle parameter = nullptr;
        const auto write = [&](const char* name, auto... args) {
            if (parameters->paramGetHandle(set, name, &parameter, nullptr) != kOfxStatOK) return false;
            return parameters->paramSetValue(parameter, args...) == kOfxStatOK
                && parameters->paramSetValueAtTime(parameter, 7.5, args...) == kOfxStatOK;
        };
        if (!write("Integer", 17) || !write("Boolean", 1) || !write("Choice", 3) || !write("Button", 0)
            || !write("Double", 2.25) || !write("String", "hello") || !write("Custom", "serialized")
            || !write("Integer2D", -2, 9) || !write("Integer3D", 1, 2, 3)
            || !write("Double2D", 0.25, 0.75) || !write("Double3D", 1.5, 2.5, 3.5)
            || !write("RGB", 0.1, 0.2, 0.3) || !write("RGBA", 0.1, 0.2, 0.3, 0.4)) return kOfxStatFailed;
        parameters->paramGetHandle(set, "Unknown", &parameter, nullptr);
        if (parameters->paramSetValue(parameter, 1) != kOfxStatErrUnsupported
            || parameters->paramSetValueAtTime(parameter, 1.0, 1) != kOfxStatErrUnsupported) return kOfxStatFailed;
        return kOfxStatOK;
    }
    if (!std::strcmp(action, "MockNSetterError")) {
        OfxPropertySetHandle props = nullptr;
        images->getPropertySet(effect, &props);
        const char* values[] = {"first", "second"};
        return properties->propSetStringN(props, "MockNFailure", 2, values);
    }
    if (!std::strcmp(action, "MockNSetterMissing")) {
        OfxPropertySetHandle props = nullptr;
        images->getPropertySet(effect, &props);
        const double values[] = {0.25, 0.75};
        return properties->propSetDoubleN(props, "MockNMissing", 2, values);
    }
    if (!std::strcmp(action, kOfxImageEffectActionRender)) {
        ++render_calls;
        int write_attempt = 0;
        properties->propGetInt(in, "MockWriteInRender", 0, &write_attempt);
        if (write_attempt) {
            OfxParamSetHandle set = nullptr;
            OfxParamHandle parameter = nullptr;
            images->getParamSet(effect, &set);
            parameters->paramGetHandle(set, "Integer", &parameter, nullptr);
            if (parameters->paramSetValue(parameter, 99) != kOfxStatErrUnsupported
                || parameters->paramSetValueAtTime(parameter, 4.0, 99) != kOfxStatErrUnsupported) return kOfxStatFailed;
            OfxPropertySetHandle parameter_props = nullptr, effect_props = nullptr;
            parameters->paramGetPropertySet(parameter, &parameter_props);
            images->getPropertySet(effect, &effect_props);
            if (properties->propSetString(parameter_props, kOfxPropLabel, 0, "forbidden render label") != kOfxStatErrUnsupported
                || properties->propReset(parameter_props, kOfxPropLabel) != kOfxStatErrUnsupported
                || properties->propSetInt(effect_props, "OfxPropParamSetNeedsSyncing", 0, 1) != kOfxStatOK)
                return kOfxStatFailed;
            return kOfxStatOK;
        }
        OfxImageClipHandle source = nullptr, output = nullptr;
        OfxPropertySetHandle source_image = nullptr, output_image = nullptr;
        images->clipGetHandle(effect, "Source", &source, nullptr);
        images->clipGetHandle(effect, "Output", &output, nullptr);
        OfxStatus result = images->clipGetImage(source, 0, nullptr, &source_image);
        if (result == kOfxStatOK) result = images->clipGetImage(output, 0, nullptr, &output_image);
        if (result == kOfxStatOK) {
            void* src = nullptr;
            void* dst = nullptr;
            void* queue = nullptr;
            properties->propGetPointer(source_image, kOfxImagePropData, 0, &src);
            properties->propGetPointer(output_image, kOfxImagePropData, 0, &dst);
            properties->propGetPointer(in, "OfxImageEffectPropMetalCommandQueue", 0, &queue);
            int source_stride = 0, output_stride = 0;
            const char* depth = nullptr;
            properties->propGetInt(source_image, kOfxImagePropRowBytes, 0, &source_stride);
            properties->propGetInt(output_image, kOfxImagePropRowBytes, 0, &output_stride);
            properties->propGetString(output_image, kOfxImageEffectPropPixelDepth, 0, &depth);
            const int bpp = depth && !std::strcmp(depth, kOfxBitDepthByte) ? 4
                : depth && (!std::strcmp(depth, kOfxBitDepthShort) || !std::strcmp(depth, "OfxBitDepthHalf")) ? 8
                : depth && !std::strcmp(depth, kOfxBitDepthFloat) ? 16 : 0;
            result = copy && copy(src, dst, queue, 8, 4, source_stride, output_stride, bpp) == 0 ? kOfxStatOK : kOfxStatFailed;
        }
        if (output_image) images->clipReleaseImage(output_image);
        if (source_image) images->clipReleaseImage(source_image);
        return result;
    }
    return kOfxStatOK;
}
OfxPlugin plugin{kOfxImageEffectPluginApi, 1, "test.mock-inner", 0, 1, set_host, entry};
}
extern "C" __attribute__((visibility("default"))) int OfxGetNumberOfPlugins() { return 1; }
extern "C" __attribute__((visibility("default"))) OfxPlugin* OfxGetPlugin(int index) { return index == 0 ? &plugin : nullptr; }
extern "C" __attribute__((visibility("default"))) unsigned MockRenderCalls() { return render_calls; }
extern "C" __attribute__((visibility("default"))) unsigned MockActionCalls() { return action_calls; }
