// SPDX-License-Identifier: GPL-3.0-or-later
// This outer adapter validates the host contract; the shared Rust library stabilizes frames.
#include <ofxCore.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#include "metal_guard.h"
#include "source_path.h"
#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <filesystem>
#include <limits>
#include <mutex>
#include <string>

namespace {
constexpr const char* adapter_flag = "NiYienJianyingMacMetalAdapter";
constexpr const char* framebuffer_flag = "NiYienJianyingMacFramebufferInverted";
constexpr const char* copy_property = "NiYienJianyingMacMetalCopyV1";
constexpr const char* source_provider_flag = "NiYienJianyingMacSourceProviderV1";
constexpr const char* source_path_property = "NiYienJianyingSourcePath";
OfxPlugin* inner = nullptr;
OfxHost* host = nullptr;
OfxHost forwarded_host{};
const OfxPropertySuiteV1* properties = nullptr;
const OfxImageEffectSuiteV1* images = nullptr;
const OfxParameterSuiteV1* parameters = nullptr;
OfxPropertySuiteV1 property_proxy{};
OfxImageEffectSuiteV1 image_proxy{};
OfxParameterSuiteV1 parameter_proxy{};
thread_local bool in_render = false;
thread_local void* render_queue = nullptr;
thread_local unsigned parameter_write_attempts = 0;
thread_local unsigned property_write_attempts = 0;
thread_local unsigned acquired_images = 0;
thread_local unsigned released_images = 0;
thread_local const char* current_action = nullptr;
thread_local const void* current_effect = nullptr;
thread_local std::string* current_source_value = nullptr;

struct ActionScope {
    const char* previous;
    const void* previous_effect;
    std::string* previous_source_value;
    std::string source_value;
    ActionScope(const char* value, const void* effect)
        : previous(current_action), previous_effect(current_effect), previous_source_value(current_source_value) {
        current_action = value;
        current_effect = effect;
        current_source_value = &source_value;
    }
    ~ActionScope() {
        current_action = previous;
        current_effect = previous_effect;
        current_source_value = previous_source_value;
    }
};

template<class... Args> void report_error(const char* format, Args... args) {
    std::fputs("Gyroflow OFX: ", stderr);
    if constexpr (sizeof...(args) == 0) std::fputs(format, stderr);
    else std::fprintf(stderr, format, args...);
    std::fputc('\n', stderr);
}

bool same(const char* a, const char* b) { return a && !std::strcmp(a, b); }
bool private_property(OfxPropertySetHandle set, const char* key) {
    return host && set == host->host && (same(key, adapter_flag) || same(key, framebuffer_flag)
        || same(key, copy_property) || same(key, source_provider_flag));
}
OfxStatus source_property_scope(OfxPropertySetHandle set) {
    const void* effect = current_effect;
    std::string* value_slot = current_source_value;
    if (!effect || !value_slot || !images || !images->getPropertySet) return kOfxStatErrUnknown;
    OfxPropertySetHandle actual = nullptr;
    if (images->getPropertySet(reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(effect)), &actual) != kOfxStatOK
        || !actual || actual != set || current_effect != effect || current_source_value != value_slot)
        return kOfxStatErrUnknown;
    return kOfxStatOK;
}
bool property_write_allowed(OfxPropertySetHandle set, const char* key) {
    if (private_property(set, key) || same(key, source_provider_flag) || same(key, source_path_property)) return false;
    // This effect notification requests a later legal action; it is not a parameter value.
    if (in_render && !same(key, "OfxPropParamSetNeedsSyncing")) {
        ++property_write_attempts;
        report_error("blocked_render_property_write property=%s", key ? key : "null");
        return false;
    }
    return true;
}
OfxStatus property_write_result(const char* operation, const char* key, OfxStatus status) {
    if (!in_render && status != kOfxStatOK)
        report_error("property_write_result action=%s operation=%s property=%s status=%d",
               current_action ? current_action : "none", operation, key ? key : "null", status);
    return status;
}

extern "C" std::int32_t copy_metal(void* source, void* destination, void* queue, std::int32_t width,
    std::int32_t height, std::int32_t source_stride, std::int32_t destination_stride, std::int32_t bytes_per_pixel) {
    const auto result = gf_jianying_metal_copy_v1(source, destination, queue, width, height,
                                                source_stride, destination_stride, bytes_per_pixel);
    if (result != 0) report_error("metal_passthrough status=%d size=%dx%d src_stride=%d dst_stride=%d reason=%s", result,
           width, height, source_stride, destination_stride, result ? gf_metal_last_error() : "completed");
    return result;
}

OfxStatus get_int(OfxPropertySetHandle set, const char* key, int index, int* value) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (same(key, source_path_property))
        return source_property_scope(set) == kOfxStatOK ? kOfxStatErrValue : kOfxStatErrUnknown;
    if (private_property(set, key)) {
        if (!value || index != 0) return kOfxStatErrBadIndex;
        if (same(key, copy_property)) return kOfxStatErrValue;
        *value = same(key, source_provider_flag) ? (jianying_macos_source_path::module_supported() ? 1 : 0)
            : same(key, adapter_flag) ? 1 : 0;
        return kOfxStatOK;
    }
    return properties->propGetInt(set, key, index, value);
}
OfxStatus get_pointer(OfxPropertySetHandle set, const char* key, int index, void** value) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (same(key, source_path_property))
        return source_property_scope(set) == kOfxStatOK ? kOfxStatErrValue : kOfxStatErrUnknown;
    if (private_property(set, key)) {
        if (!value || index != 0) return kOfxStatErrBadIndex;
        if (!same(key, copy_property)) return kOfxStatErrValue;
        *value = reinterpret_cast<void*>(&copy_metal);
        return kOfxStatOK;
    }
    return properties->propGetPointer(set, key, index, value);
}
OfxStatus get_string(OfxPropertySetHandle set, const char* key, int index, const char** value) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (private_property(set, key)) return kOfxStatErrValue;
    if (!same(key, source_path_property)) return properties->propGetString(set, key, index, value);
    if (source_property_scope(set) != kOfxStatOK) return kOfxStatErrUnknown;
    if (!value) return kOfxStatErrValue;
    *value = nullptr;
    if (index != 0) return kOfxStatErrBadIndex;
    const auto observation = jianying_macos_source_path::inspect_source(current_effect);
    current_source_value->clear();
    if (observation.status == jianying_macos_source_path::Status::Ready) *current_source_value = observation.source;
    else if (!jianying_macos_source_path::is_pending(observation)) return kOfxStatErrUnknown;
    *value = current_source_value->c_str();
    return kOfxStatOK;
}
OfxStatus get_double(OfxPropertySetHandle set, const char* key, int index, double* value) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (private_property(set, key)) return kOfxStatErrValue;
    if (same(key, source_path_property))
        return source_property_scope(set) == kOfxStatOK ? kOfxStatErrValue : kOfxStatErrUnknown;
    return properties->propGetDouble(set, key, index, value);
}
OfxStatus get_string_n(OfxPropertySetHandle set, const char* key, int count, const char** values) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (private_property(set, key)) return kOfxStatErrValue;
    if (same(key, source_path_property)) {
        if (source_property_scope(set) != kOfxStatOK) return kOfxStatErrUnknown;
        return count == 1 ? get_string(set, key, 0, values) : kOfxStatErrBadIndex;
    }
    if (!properties->propGetStringN) return kOfxStatErrMissingHostFeature;
    return properties->propGetStringN(set, key, count, values);
}
OfxStatus get_double_n(OfxPropertySetHandle set, const char* key, int count, double* values) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (private_property(set, key)) return kOfxStatErrValue;
    if (same(key, source_path_property))
        return source_property_scope(set) == kOfxStatOK ? kOfxStatErrValue : kOfxStatErrUnknown;
    if (!properties->propGetDoubleN) return kOfxStatErrMissingHostFeature;
    return properties->propGetDoubleN(set, key, count, values);
}
OfxStatus get_int_n(OfxPropertySetHandle set, const char* key, int count, int* values) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (same(key, source_path_property)) return get_int(set, key, 0, values);
    if (private_property(set, key)) return count == 1 ? get_int(set, key, 0, values) : kOfxStatErrBadIndex;
    if (!properties->propGetIntN) return kOfxStatErrMissingHostFeature;
    return properties->propGetIntN(set, key, count, values);
}
OfxStatus get_pointer_n(OfxPropertySetHandle set, const char* key, int count, void** values) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (same(key, source_path_property)) return get_pointer(set, key, 0, values);
    if (private_property(set, key)) return count == 1 ? get_pointer(set, key, 0, values) : kOfxStatErrBadIndex;
    if (!properties->propGetPointerN) return kOfxStatErrMissingHostFeature;
    return properties->propGetPointerN(set, key, count, values);
}
OfxStatus dimension(OfxPropertySetHandle set, const char* key, int* count) {
    if (same(key, source_provider_flag) && !private_property(set, key)) return kOfxStatErrUnknown;
    if (same(key, source_path_property)) {
        if (source_property_scope(set) != kOfxStatOK) return kOfxStatErrUnknown;
        if (!count) return kOfxStatErrValue;
        *count = 1;
        return kOfxStatOK;
    }
    if (private_property(set, key)) { if (!count) return kOfxStatErrValue; *count = 1; return kOfxStatOK; }
    return properties->propGetDimension(set, key, count);
}

OfxStatus set_int(OfxPropertySetHandle set, const char* key, int index, int value) {
    if (!property_write_allowed(set, key)) return kOfxStatErrUnsupported;
    const auto status = properties->propSetInt(set, key, index, value);
    return property_write_result("propSetInt", key, status);
}
OfxStatus set_pointer(OfxPropertySetHandle set, const char* key, int index, void* value) {
    return property_write_allowed(set, key) ? property_write_result("propSetPointer", key, properties->propSetPointer(set, key, index, value)) : kOfxStatErrUnsupported;
}
OfxStatus set_string(OfxPropertySetHandle set, const char* key, int index, const char* value) {
    return property_write_allowed(set, key) ? property_write_result("propSetString", key, properties->propSetString(set, key, index, value)) : kOfxStatErrUnsupported;
}
OfxStatus set_double(OfxPropertySetHandle set, const char* key, int index, double value) {
    return property_write_allowed(set, key) ? property_write_result("propSetDouble", key, properties->propSetDouble(set, key, index, value)) : kOfxStatErrUnsupported;
}
OfxStatus set_int_n(OfxPropertySetHandle set, const char* key, int count, const int* value) {
    if (!property_write_allowed(set, key)) return kOfxStatErrUnsupported;
    if (!properties->propSetIntN) return property_write_result("propSetIntN", key, kOfxStatErrMissingHostFeature);
    const auto status = properties->propSetIntN(set, key, count, value);
    return property_write_result("propSetIntN", key, status);
}
OfxStatus set_pointer_n(OfxPropertySetHandle set, const char* key, int count, void* const* value) {
    if (!property_write_allowed(set, key)) return kOfxStatErrUnsupported;
    if (!properties->propSetPointerN) return property_write_result("propSetPointerN", key, kOfxStatErrMissingHostFeature);
    return property_write_result("propSetPointerN", key, properties->propSetPointerN(set, key, count, value));
}
OfxStatus set_string_n(OfxPropertySetHandle set, const char* key, int count, const char* const* value) {
    if (!property_write_allowed(set, key)) return kOfxStatErrUnsupported;
    if (!properties->propSetStringN) return property_write_result("propSetStringN", key, kOfxStatErrMissingHostFeature);
    return property_write_result("propSetStringN", key, properties->propSetStringN(set, key, count, value));
}
OfxStatus set_double_n(OfxPropertySetHandle set, const char* key, int count, const double* value) {
    if (!property_write_allowed(set, key)) return kOfxStatErrUnsupported;
    if (!properties->propSetDoubleN) return property_write_result("propSetDoubleN", key, kOfxStatErrMissingHostFeature);
    return property_write_result("propSetDoubleN", key, properties->propSetDoubleN(set, key, count, value));
}
OfxStatus reset_property(OfxPropertySetHandle set, const char* key) {
    return property_write_allowed(set, key) ? property_write_result("propReset", key, properties->propReset(set, key)) : kOfxStatErrUnsupported;
}

template<class... Args> OfxStatus set_values(OfxParamHandle parameter, OfxTime time, bool timed, Args... args) {
    return timed ? parameters->paramSetValueAtTime(parameter, time, args...)
                 : parameters->paramSetValue(parameter, args...);
}
OfxStatus dispatch_values(OfxParamHandle parameter, OfxTime time, bool timed, va_list& args) {
    if (in_render) {
        ++parameter_write_attempts;
        report_error("blocked_parameter_value_write timed=%d", timed);
        return kOfxStatErrUnsupported;
    }
    OfxPropertySetHandle set = nullptr;
    const char* type = nullptr;
    if (parameters->paramGetPropertySet(parameter, &set) != kOfxStatOK
        || properties->propGetString(set, kOfxParamPropType, 0, &type) != kOfxStatOK) return kOfxStatErrBadHandle;
    if (same(type, kOfxParamTypeString) || same(type, kOfxParamTypeCustom))
        return set_values(parameter, time, timed, va_arg(args, const char*));
    if (same(type, kOfxParamTypeInteger) || same(type, kOfxParamTypeBoolean)
        || same(type, kOfxParamTypeChoice) || same(type, kOfxParamTypePushButton))
        return set_values(parameter, time, timed, va_arg(args, int));
    if (same(type, kOfxParamTypeInteger2D)) {
        const int a = va_arg(args, int), b = va_arg(args, int);
        return set_values(parameter, time, timed, a, b);
    }
    if (same(type, kOfxParamTypeInteger3D)) {
        const int a = va_arg(args, int), b = va_arg(args, int), c = va_arg(args, int);
        return set_values(parameter, time, timed, a, b, c);
    }
    if (same(type, kOfxParamTypeDouble)) return set_values(parameter, time, timed, va_arg(args, double));
    if (same(type, kOfxParamTypeDouble2D)) {
        const double a = va_arg(args, double), b = va_arg(args, double);
        return set_values(parameter, time, timed, a, b);
    }
    if (same(type, kOfxParamTypeDouble3D) || same(type, kOfxParamTypeRGB)) {
        const double a = va_arg(args, double), b = va_arg(args, double), c = va_arg(args, double);
        return set_values(parameter, time, timed, a, b, c);
    }
    if (same(type, kOfxParamTypeRGBA)) {
        const double a = va_arg(args, double), b = va_arg(args, double), c = va_arg(args, double), d = va_arg(args, double);
        return set_values(parameter, time, timed, a, b, c, d);
    }
    report_error("unsupported_parameter_value_type type=%s", type ? type : "null");
    return kOfxStatErrUnsupported;
}
OfxStatus set_value(OfxParamHandle parameter, ...) {
    va_list args;
    va_start(args, parameter);
    const auto result = dispatch_values(parameter, 0, false, args);
    va_end(args);
    return result;
}
OfxStatus set_value_at_time(OfxParamHandle parameter, OfxTime time, ...) {
    va_list args;
    va_start(args, time);
    const auto result = dispatch_values(parameter, time, true, args);
    va_end(args);
    return result;
}

int rgba_bytes_per_pixel(const char* depth) {
    if (same(depth, kOfxBitDepthByte)) return 4;
    if (same(depth, kOfxBitDepthShort) || same(depth, "OfxBitDepthHalf")) return 8;
    if (same(depth, kOfxBitDepthFloat)) return 16;
    return 0;
}

bool valid_image(OfxPropertySetHandle image) {
    int bounds[4]{}, stride = 0;
    const char* depth = nullptr;
    const char* components = nullptr;
    void* buffer = nullptr;
    const auto bounds_status = properties->propGetIntN
        ? properties->propGetIntN(image, kOfxImagePropBounds, 4, bounds) : kOfxStatErrMissingHostFeature;
    const auto stride_status = properties->propGetInt(image, kOfxImagePropRowBytes, 0, &stride);
    const auto depth_status = properties->propGetString(image, kOfxImageEffectPropPixelDepth, 0, &depth);
    const auto components_status = properties->propGetString(image, kOfxImageEffectPropComponents, 0, &components);
    const auto data_status = properties->propGetPointer(image, kOfxImagePropData, 0, &buffer);
    const int bytes_per_pixel = rgba_bytes_per_pixel(depth);
    if (bounds_status != kOfxStatOK || stride_status != kOfxStatOK || depth_status != kOfxStatOK
        || components_status != kOfxStatOK || data_status != kOfxStatOK
        || !bytes_per_pixel || !same(components, kOfxImageComponentRGBA)
        || bounds[0] != 0 || bounds[1] != 0 || bounds[2] <= 0 || bounds[3] <= 0
        || stride <= 0 || stride % bytes_per_pixel != 0
        || static_cast<std::uint64_t>(stride) < static_cast<std::uint64_t>(bounds[2]) * bytes_per_pixel) {
        report_error("metal_image_rejected reason=unsupported_image_properties depth=%s components=%s bounds=%d,%d,%d,%d stride=%d statuses=%d,%d,%d,%d,%d",
            depth ? depth : "null", components ? components : "null", bounds[0], bounds[1], bounds[2], bounds[3], stride,
            bounds_status, stride_status, depth_status, components_status, data_status);
        return false;
    }
    const auto required = static_cast<std::uint64_t>(bounds[3] - 1) * stride
                        + static_cast<std::uint64_t>(bounds[2]) * bytes_per_pixel;
    GFMetalBufferInfo info;
    if (required > std::numeric_limits<std::size_t>::max()
        || !gf_metal_validate_buffer(buffer, render_queue, required, &info)) {
        report_error("metal_image_rejected reason=%s", gf_metal_last_error());
        return false;
    }
    return true;
}
OfxStatus get_image(OfxImageClipHandle clip, OfxTime time, const OfxRectD* region, OfxPropertySetHandle* result) {
    if (!result) return kOfxStatErrValue;
    const auto status = images->clipGetImage(clip, time, region, result);
    if (!in_render || status != kOfxStatOK) return status;
    if (!*result || !valid_image(*result)) {
        if (*result) images->clipReleaseImage(*result);
        *result = nullptr;
        return kOfxStatErrUnsupported;
    }
    ++acquired_images;
    return status;
}
OfxStatus release_image(OfxPropertySetHandle image) {
    const auto status = images->clipReleaseImage(image);
    if (in_render && status == kOfxStatOK) ++released_images;
    return status;
}

const void* fetch_suite(OfxPropertySetHandle set, const char* name, int version) {
    const void* result = nullptr;
    if (version == 1 && same(name, kOfxPropertySuite)) result = &property_proxy;
    else if (version == 1 && same(name, kOfxImageEffectSuite)) result = &image_proxy;
    else if (version == 1 && same(name, kOfxParameterSuite)) result = &parameter_proxy;
    else result = host->fetchSuite(set, name, version);
    return result;
}
void set_host(OfxHost* value) {
    host = value;
    if (!host || !host->fetchSuite || !inner) {
        report_error("set_host_rejected reason=missing_host_fetch_or_inner");
        return;
    }
    properties = static_cast<const OfxPropertySuiteV1*>(host->fetchSuite(host->host, kOfxPropertySuite, 1));
    images = static_cast<const OfxImageEffectSuiteV1*>(host->fetchSuite(host->host, kOfxImageEffectSuite, 1));
    parameters = static_cast<const OfxParameterSuiteV1*>(host->fetchSuite(host->host, kOfxParameterSuite, 1));
    if (!properties || !images || !parameters) {
        report_error("set_host_rejected reason=missing_required_suite");
        return;
    }
    property_proxy = *properties;
    property_proxy.propGetInt = get_int;
    property_proxy.propGetPointer = get_pointer;
    property_proxy.propGetString = get_string;
    property_proxy.propGetDouble = get_double;
    property_proxy.propGetIntN = get_int_n;
    property_proxy.propGetPointerN = get_pointer_n;
    property_proxy.propGetStringN = get_string_n;
    property_proxy.propGetDoubleN = get_double_n;
    property_proxy.propGetDimension = dimension;
    property_proxy.propSetInt = set_int;
    property_proxy.propSetPointer = set_pointer;
    property_proxy.propSetString = set_string;
    property_proxy.propSetDouble = set_double;
    property_proxy.propSetIntN = set_int_n;
    property_proxy.propSetPointerN = set_pointer_n;
    property_proxy.propSetStringN = set_string_n;
    property_proxy.propSetDoubleN = set_double_n;
    property_proxy.propReset = reset_property;
    image_proxy = *images;
    image_proxy.clipGetImage = get_image;
    image_proxy.clipReleaseImage = release_image;
    parameter_proxy = *parameters;
    parameter_proxy.paramSetValue = set_value;
    parameter_proxy.paramSetValueAtTime = set_value_at_time;
    forwarded_host = *host;
    forwarded_host.fetchSuite = fetch_suite;
    inner->setHost(&forwarded_host);
}

bool metal_render_contract(OfxPropertySetHandle in, void** queue) {
    int metal = 0, other = 0;
    if (!in || properties->propGetInt(in, "OfxImageEffectPropMetalEnabled", 0, &metal) != kOfxStatOK || metal != 1)
        return false;
    if (properties->propGetInt(in, "OfxImageEffectPropOpenGLEnabled", 0, &other) != kOfxStatOK || other != 0)
        return false;
    for (auto key : {"OfxImageEffectPropOpenCLEnabled",
                     "OfxImageEffectPropCudaEnabled", "OfxImageEffectPropMetalTextureEnabled", "OfxImageEffectPropAngleEnabled"}) {
        const auto status = properties->propGetInt(in, key, 0, &other);
        if (status == kOfxStatOK && other != 0) return false;
        if (status != kOfxStatOK && status != kOfxStatErrUnknown) return false;
    }
    return properties->propGetPointer(in, "OfxImageEffectPropMetalCommandQueue", 0, queue) == kOfxStatOK && *queue;
}

void define_diagnostic_mode(OfxImageEffectHandle effect) {
    OfxParamSetHandle set = nullptr;
    OfxPropertySetHandle property = nullptr;
    if (images->getParamSet(effect, &set) != kOfxStatOK
        || parameters->paramDefine(set, kOfxParamTypeChoice, "DiagnosticMode", &property) != kOfxStatOK) return;
    properties->propSetString(property, kOfxPropLabel, 0, "Diagnostic mode");
    properties->propSetString(property, kOfxParamPropHint, 0, "Internal rendering diagnostics.");
    properties->propSetInt(property, kOfxParamPropDefault, 0, 0);
    properties->propSetInt(property, kOfxParamPropAnimates, 0, 0);
    properties->propSetInt(property, kOfxParamPropSecret, 0, 1);
    const char* choices[] = {"0 - Stabilize", "1 - Source copy", "2 - Color pattern", "3 - Source opaque", "4 - Batch sweep (2.5s each)"};
    properties->propSetStringN(property, kOfxParamPropChoiceOption, 5, choices);
    OfxParamHandle page = nullptr;
    OfxPropertySetHandle page_properties = nullptr;
    if (parameters->paramGetHandle(set, "Main", &page, &page_properties) == kOfxStatOK) {
        int count = 0;
        properties->propGetDimension(page_properties, kOfxParamPropPageChild, &count);
        properties->propSetString(page_properties, kOfxParamPropPageChild, count, "DiagnosticMode");
    }
}

OfxStatus entry(const char* action, const void* handle, OfxPropertySetHandle in, OfxPropertySetHandle out) {
    ActionScope action_scope(action, handle);
    const bool render = same(action, kOfxImageEffectActionRender);
    if (!inner || !host || !properties || !images || !parameters) {
        report_error("action_rejected action=%s status=%d reason=missing_host_or_suite", action ? action : "null", kOfxStatErrMissingHostFeature);
        return kOfxStatErrMissingHostFeature;
    }
    const char* name = nullptr;
    const auto host_name_status = properties->propGetString(host->host, kOfxPropName, 0, &name);
    if (host_name_status != kOfxStatOK || !same(name, "com.lemon.lvpro")) {
        report_error("action_rejected action=%s status=%d reason=host_identity", action ? action : "null", kOfxStatErrUnsupported);
        return kOfxStatErrUnsupported;
    }
    if (same(action, "OfxActionMetalContextAttached") || same(action, "OfxActionMetalContextDetached")) {
        // The pinned inner binding predates these actions; its Metal device is initialized from Render's host queue.
        return kOfxStatReplyDefault;
    }
    if (same(action, kOfxActionBeginInstanceEdit) || same(action, kOfxActionEndInstanceEdit)) {
        // The pinned binding rejects these optional UI notifications before plugin dispatch.
        return kOfxStatReplyDefault;
    }
    if (render) {
        void* queue = nullptr;
        if (!metal_render_contract(in, &queue)) {
            report_error("render_rejected reason=requires_standard_metal_buffers");
            return kOfxStatErrUnsupported;
        }
        in_render = true;
        render_queue = queue;
        parameter_write_attempts = property_write_attempts = acquired_images = released_images = 0;
    }
    auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(handle));
    const auto status = inner->mainEntry(action, handle, in, out);
    if (same(action, kOfxActionDescribe) && status == kOfxStatOK) {
        OfxPropertySetHandle descriptor = nullptr;
        if (images->getPropertySet(effect, &descriptor) == kOfxStatOK) {
            for (auto key : {kOfxPropLabel, kOfxPropShortLabel, kOfxPropLongLabel})
                properties->propSetString(descriptor, key, 0, "Gyroflow");
            properties->propSetString(descriptor, kOfxImageEffectPluginPropGrouping, 0, "NiYien");
        }
    }
    if (same(action, kOfxImageEffectActionDescribeInContext) && status == kOfxStatOK) define_diagnostic_mode(effect);
    if (render) {
        in_render = false;
        render_queue = nullptr;
        if (parameter_write_attempts || property_write_attempts || acquired_images != released_images) return kOfxStatFailed;
    }
    return status;
}

OfxPlugin plugin{kOfxImageEffectPluginApi, 1, "xyz.NiYien.gyroflow", 1, 11, set_host, entry};
}

extern "C" __attribute__((visibility("default"))) int OfxGetNumberOfPlugins() {
    return 1;
}
extern "C" __attribute__((visibility("default"))) OfxPlugin* OfxGetPlugin(int index) {
    if (index != 0) return nullptr;
    static std::once_flag once;
    std::call_once(once, [] {
        Dl_info module{};
        if (!dladdr(reinterpret_cast<void*>(&OfxGetPlugin), &module) || !module.dli_fname) return;
        const auto directory = std::filesystem::path(module.dli_fname).parent_path();
        auto path = directory.parent_path() / "PlugIns/GyroflowNiyien.ofx.bundle/Contents/MacOS/GyroflowNiyien.ofx";
        if (!std::filesystem::exists(path)) path = directory / "libgyroflow_ofx.dylib";
        void* library = dlopen(path.c_str(), RTLD_LOCAL | RTLD_NOW);
        if (!library) { report_error("load_inner_failed reason=%s", dlerror()); return; }
        auto get = reinterpret_cast<OfxPlugin* (*)(int)>(dlsym(library, "OfxGetPlugin"));
        if (get) inner = get(0);
    });
    return inner ? &plugin : nullptr;
}
