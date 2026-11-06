// SPDX-License-Identifier: GPL-3.0-or-later
// This outer adapter validates the host contract; the shared Rust library stabilizes frames.
#include <ofxCore.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#include "metal_guard.h"
#include "source_path.h"
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <filesystem>
#include <limits>
#include <iomanip>
#include <locale>
#include <map>
#include <mutex>
#include <pthread.h>
#include <sstream>
#include <string>
#include <unistd.h>

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
std::mutex log_mutex;
std::atomic<unsigned long long> sequence{0};
std::atomic<unsigned long long> snapshot_sequence{0};
std::atomic<unsigned long long> action_sequence{0};
thread_local bool in_render = false;
thread_local void* render_queue = nullptr;
thread_local unsigned long long render_id = 0;
thread_local unsigned parameter_write_attempts = 0;
thread_local unsigned property_write_attempts = 0;
thread_local unsigned acquired_images = 0;
thread_local unsigned released_images = 0;
thread_local const char* current_action = nullptr;
thread_local unsigned long long current_action_id = 0;
thread_local unsigned current_action_depth = 0;
thread_local const void* current_effect = nullptr;
thread_local std::string* current_source_value = nullptr;
thread_local bool source_probe_active = false;
thread_local bool snapshot_active = false;

struct ActionScope {
    const char* previous;
    unsigned long long previous_id;
    unsigned previous_depth;
    const void* previous_effect;
    std::string* previous_source_value;
    std::string source_value;
    ActionScope(const char* value, const void* effect)
        : previous(current_action), previous_id(current_action_id), previous_depth(current_action_depth),
          previous_effect(current_effect), previous_source_value(current_source_value) {
        current_action = value;
        current_action_id = ++action_sequence;
        current_action_depth = previous_depth + 1;
        current_effect = effect;
        current_source_value = &source_value;
    }
    ~ActionScope() {
        current_action = previous;
        current_action_id = previous_id;
        current_action_depth = previous_depth;
        current_effect = previous_effect;
        current_source_value = previous_source_value;
    }
};
struct SnapshotScope {
    bool previous;
    SnapshotScope() : previous(snapshot_active) { snapshot_active = true; }
    ~SnapshotScope() { snapshot_active = previous; }
};

template<class... Args> void report(const char* format, Args... args) {
    std::lock_guard<std::mutex> lock(log_mutex);
    std::filesystem::path path;
    const char* override_path = std::getenv("NIYIEN_JIANYING_MAC_LOG");
    if (override_path && std::filesystem::path(override_path).is_absolute()) path = override_path;
    else {
        const char* home = std::getenv("HOME");
        if (!home) return;
        path = std::filesystem::path(home) / "Library/Logs/Gyroflow/jianying-macos-candidate.log";
    }
    std::error_code error;
    std::filesystem::create_directories(path.parent_path(), error);
    FILE* file = std::fopen(path.c_str(), "a");
    if (!file) return;
    const auto timestamp = std::chrono::duration_cast<std::chrono::microseconds>(
        std::chrono::system_clock::now().time_since_epoch()).count();
    uint64_t thread = 0;
    pthread_threadid_np(nullptr, &thread);
    std::fprintf(file, "time_us=%lld pid=%d tid=%llu action_id=%llu depth=%u effect=%p render_id=%llu ",
                 static_cast<long long>(timestamp), getpid(), static_cast<unsigned long long>(thread),
                 current_action_id, current_action_depth, current_effect, render_id);
    if constexpr (sizeof...(args) == 0) std::fputs(format, file);
    else std::fprintf(file, format, args...);
    std::fputc('\n', file);
    std::fclose(file);
}

bool same(const char* a, const char* b) { return a && !std::strcmp(a, b); }
bool extended_ui_diagnostics() {
    static const bool enabled = [] {
        const char* value = std::getenv("NIYIEN_JIANYING_MAC_OBSERVE_UI");
        return value && !std::strcmp(value, "1");
    }();
    return enabled;
}
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
        report("blocked_render_property_write property=%s", key ? key : "null");
        return false;
    }
    return true;
}
OfxStatus property_write_result(const char* operation, const char* key, OfxStatus status) {
    if (!in_render && status != kOfxStatOK)
        report("property_write_result action=%s operation=%s property=%s status=%d",
               current_action ? current_action : "none", operation, key ? key : "null", status);
    return status;
}

extern "C" std::int32_t copy_metal(void* source, void* destination, void* queue, std::int32_t width,
    std::int32_t height, std::int32_t source_stride, std::int32_t destination_stride, std::int32_t bytes_per_pixel) {
    const auto result = gf_jianying_metal_copy_v1(source, destination, queue, width, height,
                                                source_stride, destination_stride, bytes_per_pixel);
    report("metal_passthrough status=%d size=%dx%d src_stride=%d dst_stride=%d reason=%s", result,
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
        if (!in_render) report("private_get_int property=%s index=%d status=0 value=%d host=%p", key, index, *value, set);
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
        if (!in_render) report("private_get_pointer property=%s index=%d status=0 value=%p host=%p", key, index, *value, set);
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
    if (extended_ui_diagnostics() && !in_render && same(key, kOfxParamPropEnabled))
        report("parameter_enabled_write api=propSetInt property=%s property_set=%p index=%d value=%d status=%d",
               key, set, index, value, status);
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
    if (extended_ui_diagnostics() && !in_render && same(key, kOfxParamPropEnabled))
        report("parameter_enabled_write api=propSetIntN property=%s property_set=%p count=%d first_value_available=%d first_value=%d status=%d",
               key, set, count, count > 0 && value, count > 0 && value ? value[0] : 0, status);
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
        report("blocked_parameter_value_write timed=%d", timed);
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
    report("unsupported_parameter_value_type type=%s", type ? type : "null");
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

bool valid_image(OfxPropertySetHandle image) {
    int bounds[4]{}, stride = 0;
    const char* depth = nullptr;
    const char* components = nullptr;
    void* buffer = nullptr;
    if (!properties->propGetIntN || properties->propGetIntN(image, kOfxImagePropBounds, 4, bounds) != kOfxStatOK
        || properties->propGetInt(image, kOfxImagePropRowBytes, 0, &stride) != kOfxStatOK
        || properties->propGetString(image, kOfxImageEffectPropPixelDepth, 0, &depth) != kOfxStatOK
        || properties->propGetString(image, kOfxImageEffectPropComponents, 0, &components) != kOfxStatOK
        || properties->propGetPointer(image, kOfxImagePropData, 0, &buffer) != kOfxStatOK
        || !same(depth, kOfxBitDepthByte) || !same(components, kOfxImageComponentRGBA)
        || bounds[0] != 0 || bounds[1] != 0 || bounds[2] <= 0 || bounds[3] <= 0
        || stride <= 0 || stride % 4 != 0
        || static_cast<std::uint64_t>(stride) < static_cast<std::uint64_t>(bounds[2]) * 4) {
        report("metal_image_rejected reason=unsupported_image_properties");
        return false;
    }
    const auto required = static_cast<std::uint64_t>(bounds[3] - 1) * stride
                        + static_cast<std::uint64_t>(bounds[2]) * 4;
    GFMetalBufferInfo info;
    if (required > std::numeric_limits<std::size_t>::max()
        || !gf_metal_validate_buffer(buffer, render_queue, required, &info)) {
        report("metal_image_rejected reason=%s", gf_metal_last_error());
        return false;
    }
    report("metal_image_valid size=%dx%d stride=%d length=%zu storage=%lu same_device=1",
           bounds[2], bounds[3], stride, info.length, info.storage_mode);
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
    if (!in_render) report("suite_fetch name=%s version=%d set=%p result=%p", name ? name : "null", version, set, result);
    return result;
}
void set_host(OfxHost* value) {
    report("set_host_begin host_struct=%p inner=%p revision=candidate-02-diagnostics", value, inner);
    host = value;
    if (!host || !host->fetchSuite || !inner) {
        report("set_host_rejected reason=missing_host_fetch_or_inner");
        return;
    }
    properties = static_cast<const OfxPropertySuiteV1*>(host->fetchSuite(host->host, kOfxPropertySuite, 1));
    images = static_cast<const OfxImageEffectSuiteV1*>(host->fetchSuite(host->host, kOfxImageEffectSuite, 1));
    parameters = static_cast<const OfxParameterSuiteV1*>(host->fetchSuite(host->host, kOfxParameterSuite, 1));
    report("set_host_suites host_property=%p property=%p image=%p parameter=%p", host->host, properties, images, parameters);
    if (!properties || !images || !parameters) {
        report("set_host_rejected reason=missing_required_suite");
        return;
    }
    report("set_host_property_functions set_string_n=%p set_int_n=%p set_double_n=%p set_pointer_n=%p get_int_n=%p get_pointer_n=%p",
           reinterpret_cast<void*>(properties->propSetStringN), reinterpret_cast<void*>(properties->propSetIntN),
           reinterpret_cast<void*>(properties->propSetDoubleN), reinterpret_cast<void*>(properties->propSetPointerN),
           reinterpret_cast<void*>(properties->propGetIntN), reinterpret_cast<void*>(properties->propGetPointerN));
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
    const char* name = nullptr;
    const auto status = properties->propGetString(host->host, kOfxPropName, 0, &name);
    report("set_host_end host_name_status=%d host_name=%s forwarded_host=%p", status, name ? name : "null", &forwarded_host);
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

std::string quoted_value(const char* text) {
    if (!text) return "null";
    std::string result = "\"";
    for (const unsigned char character : std::string(text)) {
        if (character == '"' || character == '\\') {
            result += '\\';
            result += static_cast<char>(character);
        } else if (character < 32) {
            char escape[7];
            std::snprintf(escape, sizeof(escape), "\\u%04x", static_cast<unsigned>(character));
            result += escape;
        } else result += static_cast<char>(character);
    }
    return result + '"';
}

struct SourceObservation {
    int scope_status = -1;
    int dimension_status = -1;
    int dimension = -1;
    int getter_status = -1;
    bool dimension_attempted = false;
    bool getter_attempted = false;
    bool value_present = false;
    bool value_over_limit = false;
    std::string value;
    bool operator==(const SourceObservation& other) const {
        return scope_status == other.scope_status && dimension_status == other.dimension_status
            && dimension == other.dimension && getter_status == other.getter_status
            && dimension_attempted == other.dimension_attempted && getter_attempted == other.getter_attempted
            && value_present == other.value_present && value_over_limit == other.value_over_limit
            && value == other.value;
    }
};
std::mutex source_cache_mutex;
std::map<const void*, std::map<std::string, SourceObservation>> source_observations;
std::map<const void*, jianying_macos_source_path::Observation> native_source_observations;

void record_source_observation(OfxImageEffectHandle effect, const char* action, const char* scope,
                               const char* key, OfxPropertySetHandle property_set, OfxImageClipHandle clip,
                               const SourceObservation& observation) {
    bool changed = false;
    {
        // Never hold this cache lock while calling a host suite or writing the log.
        std::lock_guard<std::mutex> lock(source_cache_mutex);
        auto& entries = source_observations[effect];
        const auto cache_key = std::string(scope) + "." + key;
        const auto previous = entries.find(cache_key);
        changed = previous == entries.end() || !(previous->second == observation);
        if (changed) entries[cache_key] = observation;
    }
    if (!changed) return;
    report("source_property action=%s scope=%s property=%s property_set=%p source_clip=%p scope_status=%d dimension_attempted=%d dimension_status=%d dimension=%d getter_attempted=%d getter_status=%d value_present=%d value_over_limit=%d value_json=%s",
           action, scope, key, property_set, clip, observation.scope_status, observation.dimension_attempted,
           observation.dimension_status, observation.dimension, observation.getter_attempted, observation.getter_status,
           observation.value_present, observation.value_over_limit,
           observation.value_present ? quoted_value(observation.value.c_str()).c_str() : "null");
}

void probe_source_scope(OfxImageEffectHandle effect, const char* action, const char* scope,
                        int scope_status, OfxPropertySetHandle property_set, OfxImageClipHandle clip) {
    // These optional host extensions have string values; never reinterpret another property type.
    const char* keys[] = {"OfxImageEffectPropSrcFilePath", "OfxImageEffectPropSourceFilePath",
        "OfxImageClipPropFilePath", "OfxImageClipPropFileName", "OfxImageEffectPropFilePath", "OfxImagePropFilePath"};
    for (const char* key : keys) {
        SourceObservation observation;
        observation.scope_status = scope_status;
        if (scope_status == kOfxStatOK && property_set) {
            observation.dimension_attempted = true;
            observation.dimension_status = properties->propGetDimension
                ? properties->propGetDimension(property_set, key, &observation.dimension) : kOfxStatErrMissingHostFeature;
            // Getter and dimension support are independent for optional host extensions.
            const char* value = nullptr;
            observation.getter_attempted = true;
            observation.getter_status = properties->propGetString
                ? properties->propGetString(property_set, key, 0, &value) : kOfxStatErrMissingHostFeature;
            if (observation.getter_status == kOfxStatOK && value) {
                constexpr std::size_t maximum_observed_path_bytes = 32767;
                const auto length = strnlen(value, maximum_observed_path_bytes + 1);
                observation.value_over_limit = length > maximum_observed_path_bytes;
                if (!observation.value_over_limit) {
                    observation.value.assign(value, length);
                    observation.value_present = true;
                }
            }
        }
        record_source_observation(effect, action, scope, key, property_set, clip, observation);
    }
}

void probe_source_properties(OfxImageEffectHandle effect, const char* action) {
    if (!effect || in_render || source_probe_active) return;
    struct ProbeScope {
        ProbeScope() { source_probe_active = true; }
        ~ProbeScope() { source_probe_active = false; }
    } scope;
    OfxPropertySetHandle effect_properties = nullptr;
    const auto effect_status = images->getPropertySet
        ? images->getPropertySet(effect, &effect_properties) : kOfxStatErrMissingHostFeature;
    probe_source_scope(effect, action, "effect", effect_status, effect_properties, nullptr);
    OfxImageClipHandle source = nullptr;
    OfxPropertySetHandle source_properties = nullptr;
    const auto source_status = images->clipGetHandle
        ? images->clipGetHandle(effect, kOfxImageEffectSimpleSourceClipName, &source, &source_properties)
        : kOfxStatErrMissingHostFeature;
    probe_source_scope(effect, action, "source_clip", source_status, source_properties, source);
}

void observe_native_source(OfxImageEffectHandle effect, const char* action) {
    if (!effect) return;
    const auto observation = jianying_macos_source_path::inspect_source(effect);
    bool changed = false;
    {
        std::lock_guard<std::mutex> lock(source_cache_mutex);
        const auto previous = native_source_observations.find(effect);
        changed = previous == native_source_observations.end() || !(previous->second == observation);
        if (changed) native_source_observations[effect] = observation;
    }
    if (changed) report("native_source action=%s status=%s processor=%p source_bytes=%zu source_json=%s",
                        action, jianying_macos_source_path::status_name(observation.status),
                        reinterpret_cast<void*>(observation.processor), observation.source.size(),
                        observation.status == jianying_macos_source_path::Status::Ready
                            ? quoted_value(observation.source.c_str()).c_str() : "null");
}

void forget_source_observations(OfxImageEffectHandle effect) {
    std::lock_guard<std::mutex> lock(source_cache_mutex);
    source_observations.erase(effect);
    native_source_observations.erase(effect);
}

void snapshot_parameters(OfxImageEffectHandle effect, const char* action) {
    const auto snapshot_id = ++snapshot_sequence;
    if (in_render || snapshot_active) {
        report("parameter_snapshot snapshot_id=%llu stage=%s action=%s effect=%p",
               snapshot_id, in_render ? "skipped_render_scope" : "skipped_reentrant_snapshot", action, effect);
        return;
    }
    SnapshotScope scope;
    OfxParamSetHandle set = nullptr;
    const auto set_status = effect && images->getParamSet
        ? images->getParamSet(effect, &set) : kOfxStatErrBadHandle;
    report("parameter_snapshot snapshot_id=%llu stage=after_inner_success action=%s effect=%p param_set_status=%d",
           snapshot_id, action, effect, set_status);
    struct Definition { const char* name; const char* type; };
    // These types are the formal shared parameter definitions, not inferred from current values.
    const Definition definitions[] = {
        {"ProjectPath", kOfxParamTypeString}, {"Smoothness", kOfxParamTypeDouble},
        {"ZoomMode", kOfxParamTypeChoice}, {"TranslationEnabled", kOfxParamTypeBoolean},
        {"InstanceId", kOfxParamTypeString},
        {"Status", kOfxParamTypeString}, {"LoadedProject", kOfxParamTypeString},
        {"LoadedLens", kOfxParamTypeString}, {"LoadedPreset", kOfxParamTypeString},
        {"TranslationStatus", kOfxParamTypeString},
    };
    for (const auto& definition : definitions) {
        OfxParamHandle parameter = nullptr;
        OfxPropertySetHandle parameter_properties = nullptr;
        const char* actual_type = nullptr;
        int handle_status = -1, property_status = -1, type_status = -1, value_status = -1;
        bool attempted = false;
        int finite = -1;
        std::string value = "null";
        if (set_status == kOfxStatOK && set && parameters->paramGetHandle) {
            handle_status = parameters->paramGetHandle(set, definition.name, &parameter, &parameter_properties);
            if (handle_status == kOfxStatOK && parameter) {
                property_status = parameter_properties ? kOfxStatOK : parameters->paramGetPropertySet
                    ? parameters->paramGetPropertySet(parameter, &parameter_properties) : kOfxStatErrMissingHostFeature;
                if (property_status == kOfxStatOK && parameter_properties && properties->propGetString)
                    type_status = properties->propGetString(parameter_properties, kOfxParamPropType, 0, &actual_type);
                if (type_status == kOfxStatOK && same(actual_type, definition.type) && parameters->paramGetValue) {
                    attempted = true;
                    if (same(definition.type, kOfxParamTypeString)) {
                        char* text = nullptr;
                        value_status = parameters->paramGetValue(parameter, &text);
                        if (value_status == kOfxStatOK) value = quoted_value(text);
                    } else if (same(definition.type, kOfxParamTypeDouble)) {
                        double number = 0;
                        value_status = parameters->paramGetValue(parameter, &number);
                        if (value_status == kOfxStatOK) {
                            std::ostringstream text;
                            text.imbue(std::locale::classic());
                            text << std::setprecision(std::numeric_limits<double>::max_digits10) << number;
                            finite = std::isfinite(number) ? 1 : 0;
                            value = finite ? text.str() : quoted_value(text.str().c_str());
                        }
                    } else {
                        int number = 0;
                        value_status = parameters->paramGetValue(parameter, &number);
                        if (value_status == kOfxStatOK) value = std::to_string(number);
                    }
                }
            }
        }
        report("parameter_snapshot_value snapshot_id=%llu stage=after_inner_success action=%s effect=%p parameter=%s declared_type=%s actual_type_json=%s handle=%p parameter_properties=%p get_handle_status=%d get_property_status=%d get_type_status=%d value_attempted=%d value_status=%d value_finite=%d value_json=%s",
               snapshot_id, action, effect, definition.name, definition.type, quoted_value(actual_type).c_str(), parameter,
               parameter_properties, handle_status, property_status, type_status, attempted, value_status, finite, value.c_str());
        if (extended_ui_diagnostics() && (same(definition.name, "ProjectPath")
            || same(definition.name, "Smoothness") || same(definition.name, "ZoomMode"))) {
            bool evaluated_attempted = false, keys_attempted = false;
            int evaluated_status = -1, keys_status = -1, evaluated_finite = -1;
            unsigned key_count = 0;
            std::string evaluated_value = "null", keys_value = "null";
            if (type_status == kOfxStatOK && same(actual_type, definition.type) && parameter) {
                if (parameters->paramGetValueAtTime) {
                    evaluated_attempted = true;
                    if (same(definition.type, kOfxParamTypeString)) {
                        char* text = nullptr;
                        evaluated_status = parameters->paramGetValueAtTime(parameter, 0.0, &text);
                        if (evaluated_status == kOfxStatOK) evaluated_value = quoted_value(text);
                    } else if (same(definition.type, kOfxParamTypeDouble)) {
                        double number = 0;
                        evaluated_status = parameters->paramGetValueAtTime(parameter, 0.0, &number);
                        if (evaluated_status == kOfxStatOK) {
                            std::ostringstream text;
                            text.imbue(std::locale::classic());
                            text << std::setprecision(std::numeric_limits<double>::max_digits10) << number;
                            evaluated_finite = std::isfinite(number) ? 1 : 0;
                            evaluated_value = evaluated_finite ? text.str() : quoted_value(text.str().c_str());
                        }
                    } else {
                        int number = 0;
                        evaluated_status = parameters->paramGetValueAtTime(parameter, 0.0, &number);
                        if (evaluated_status == kOfxStatOK) evaluated_value = std::to_string(number);
                    }
                } else evaluated_status = kOfxStatErrMissingHostFeature;
                if (!same(definition.name, "ProjectPath") && parameters->paramGetNumKeys) {
                    keys_attempted = true;
                    keys_status = parameters->paramGetNumKeys(parameter, &key_count);
                    if (keys_status == kOfxStatOK) keys_value = std::to_string(key_count);
                } else if (!same(definition.name, "ProjectPath")) keys_status = kOfxStatErrMissingHostFeature;
            }
            report("parameter_evaluation snapshot_id=%llu stage=after_inner_success action=%s effect=%p parameter=%s declared_type=%s actual_type_json=%s time=0 value_at_time_api=paramGetValueAtTime value_at_time_attempted=%d value_at_time_status=%d value_at_time_finite=%d value_at_time_json=%s num_keys_api=paramGetNumKeys num_keys_applicable=%d num_keys_attempted=%d num_keys_status=%d num_keys_json=%s",
                   snapshot_id, action, effect, definition.name, definition.type, quoted_value(actual_type).c_str(),
                   evaluated_attempted, evaluated_status, evaluated_finite, evaluated_value.c_str(),
                   !same(definition.name, "ProjectPath"), keys_attempted, keys_status, keys_value.c_str());
        }
    }
}

void observe_change_arguments(OfxPropertySetHandle in) {
    for (const char* key : {kOfxPropName, kOfxPropChangeReason, kOfxPropType}) {
        const char* value = nullptr;
        const bool attempted = in && properties->propGetString;
        const auto status = attempted ? properties->propGetString(in, key, 0, &value)
            : in ? kOfxStatErrMissingHostFeature : kOfxStatErrBadHandle;
        report("instance_change_argument property=%s api=propGetString getter_attempted=%d getter_status=%d value_json=%s",
               key, attempted, status, status == kOfxStatOK ? quoted_value(value).c_str() : "null");
    }
    double value = 0;
    const bool attempted = in && properties->propGetDouble;
    const auto status = attempted ? properties->propGetDouble(in, kOfxPropTime, 0, &value)
        : in ? kOfxStatErrMissingHostFeature : kOfxStatErrBadHandle;
    std::string encoded = "null";
    int finite = -1;
    if (status == kOfxStatOK) {
        std::ostringstream text;
        text.imbue(std::locale::classic());
        text << std::setprecision(std::numeric_limits<double>::max_digits10) << value;
        finite = std::isfinite(value) ? 1 : 0;
        encoded = finite ? text.str() : quoted_value(text.str().c_str());
    }
    report("instance_change_argument property=%s api=propGetDouble getter_attempted=%d getter_status=%d value_finite=%d value_json=%s",
           kOfxPropTime, attempted, status, finite, encoded.c_str());
}
void observe_publisher_state(OfxImageEffectHandle effect, const char* action, const char* stage) {
    int temporal = -1;
    const bool attempted = host && host->host && properties->propGetInt;
    const auto temporal_status = attempted
        ? properties->propGetInt(host->host, kOfxImageEffectPropTemporalClipAccess, 0, &temporal)
        : kOfxStatErrMissingHostFeature;
    const auto observation = jianying_macos_source_path::inspect_publish_state(effect);
    report("native_publish_state stage=%s action=%s temporal_api=propGetInt temporal_attempted=%d temporal_status=%d temporal_value=%d external_mirror_verified=%d identity_status=%s fields_read=%d created=%u edit_count=%d plugin_changed_set_size=%llu",
           stage, action, attempted, temporal_status, temporal,
           jianying_macos_source_path::module_supported() ? 1 : 0,
           jianying_macos_source_path::status_name(observation.status), observation.fields_read,
           static_cast<unsigned>(observation.created), static_cast<int>(observation.edit_count),
           static_cast<unsigned long long>(observation.plugin_changed_size));
    const auto route = jianying_macos_source_path::inspect_route_id(effect);
    report("native_route_id stage=%s action=%s status=%s processor=%p clip_id_bytes=%zu clip_id_json=%s",
           stage, action, jianying_macos_source_path::status_name(route.status),
           reinterpret_cast<void*>(route.processor), route.clip_id.size(),
           route.status == jianying_macos_source_path::Status::Ready ? quoted_value(route.clip_id.c_str()).c_str() : "null");
}

OfxStatus entry(const char* action, const void* handle, OfxPropertySetHandle in, OfxPropertySetHandle out) {
    ActionScope action_scope(action, handle);
    const bool render = same(action, kOfxImageEffectActionRender);
    if (!render) report("action_begin action=%s handle=%p in=%p out=%p", action ? action : "null", handle, in, out);
    if (!inner || !host || !properties || !images || !parameters) {
        report("action_rejected action=%s status=%d reason=missing_host_or_suite", action ? action : "null", kOfxStatErrMissingHostFeature);
        return kOfxStatErrMissingHostFeature;
    }
    const char* name = nullptr;
    const auto host_name_status = properties->propGetString(host->host, kOfxPropName, 0, &name);
    if (!render) report("action_host action=%s name_status=%d name=%s", action ? action : "null", host_name_status, name ? name : "null");
    if (host_name_status != kOfxStatOK || !same(name, "com.lemon.lvpro")) {
        report("action_rejected action=%s status=%d reason=host_identity", action ? action : "null", kOfxStatErrUnsupported);
        return kOfxStatErrUnsupported;
    }
    if (same(action, "OfxActionMetalContextAttached") || same(action, "OfxActionMetalContextDetached")) {
        // The pinned inner binding predates these actions; its Metal device is initialized from Render's host queue.
        report("metal_context_noop action=%s status=%d forwarded=0 gpu_commands=0 parameter_writes=0",
               action, kOfxStatReplyDefault);
        report("action_end action=%s status=%d", action, kOfxStatReplyDefault);
        return kOfxStatReplyDefault;
    }
    if (same(action, kOfxActionBeginInstanceEdit) || same(action, kOfxActionEndInstanceEdit)) {
        // The pinned binding rejects these optional UI notifications before plugin dispatch.
        report("instance_edit_noop action=%s status=%d forwarded=0 parameter_reads=0 parameter_writes=0 gpu_commands=0",
               action, kOfxStatReplyDefault);
        report("action_end action=%s status=%d", action, kOfxStatReplyDefault);
        return kOfxStatReplyDefault;
    }
    if (render) {
        void* queue = nullptr;
        if (!metal_render_contract(in, &queue)) {
            report("render_rejected reason=requires_standard_metal_buffers");
            return kOfxStatErrUnsupported;
        }
        in_render = true;
        render_queue = queue;
        render_id = ++sequence;
        parameter_write_attempts = property_write_attempts = acquired_images = released_images = 0;
    }
    auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(handle));
    if (render || same(action, kOfxImageEffectActionIsIdentity)) observe_native_source(effect, action);
    if (extended_ui_diagnostics() && same(action, kOfxActionInstanceChanged)) observe_change_arguments(in);
    const bool publisher_observation = extended_ui_diagnostics()
        && (same(action, kOfxActionEndInstanceChanged) || same(action, kOfxActionSyncPrivateData));
    if (publisher_observation) observe_publisher_state(effect, action, "before_inner");
    report("action_inner_dispatch action=%s handle=%p", action ? action : "null", handle);
    const auto status = inner->mainEntry(action, handle, in, out);
    if (!render) report("action_inner_return action=%s status=%d", action ? action : "null", status);
    if (publisher_observation) observe_publisher_state(effect, action, "after_inner");
    if (same(action, kOfxActionDescribe) && status == kOfxStatOK) {
        OfxPropertySetHandle descriptor = nullptr;
        if (images->getPropertySet(effect, &descriptor) == kOfxStatOK)
            for (auto key : {kOfxPropLabel, kOfxPropShortLabel, kOfxPropLongLabel})
                properties->propSetString(descriptor, key, 0, "Gyroflow");
        report("candidate_describe host=com.lemon.lvpro metal_only=1 framebuffer_inverted=0 orientation_validation=pending");
    }
    if (same(action, kOfxImageEffectActionDescribeInContext) && status == kOfxStatOK) define_diagnostic_mode(effect);
    if (!render && status == kOfxStatOK && (current_action_depth == 1 || extended_ui_diagnostics())
        && (same(action, kOfxActionEndInstanceChanged) || same(action, kOfxActionSyncPrivateData)))
        snapshot_parameters(effect, action);
    if (extended_ui_diagnostics() && !render && status == kOfxStatOK
        && (same(action, kOfxActionCreateInstance) || same(action, kOfxActionEndInstanceChanged)
            || same(action, kOfxActionSyncPrivateData)))
        probe_source_properties(effect, action);
    if (!render && status == kOfxStatOK
        && (same(action, kOfxActionCreateInstance) || same(action, kOfxActionEndInstanceChanged)
            || same(action, kOfxActionSyncPrivateData)))
        observe_native_source(effect, action);
    if (same(action, kOfxActionDestroyInstance)) forget_source_observations(effect);
    if (!render) report("action_end action=%s status=%d", action ? action : "null", status);
    if (render) {
        report("render_end status=%d acquired=%u released=%u parameter_value_write_attempts=%u property_write_attempts=%u outer_cpu_pixel_access=0",
               status, acquired_images, released_images, parameter_write_attempts, property_write_attempts);
        in_render = false;
        render_queue = nullptr;
        if (parameter_write_attempts || property_write_attempts || acquired_images != released_images) return kOfxStatFailed;
    }
    return status;
}

OfxPlugin plugin{kOfxImageEffectPluginApi, 1, "xyz.niyien.gyroflow.jianying.batch11", 1, 11, set_host, entry};
}

extern "C" __attribute__((visibility("default"))) int OfxGetNumberOfPlugins() {
    report("get_number_of_plugins count=1 revision=candidate-02-diagnostics");
    return 1;
}
extern "C" __attribute__((visibility("default"))) OfxPlugin* OfxGetPlugin(int index) {
    if (index != 0) return nullptr;
    static std::once_flag once;
    std::call_once(once, [] {
        Dl_info module{};
        if (!dladdr(reinterpret_cast<void*>(&OfxGetPlugin), &module) || !module.dli_fname) return;
        const auto path = std::filesystem::path(module.dli_fname).parent_path() / "libgyroflow_ofx.dylib";
        void* library = dlopen(path.c_str(), RTLD_LOCAL | RTLD_NOW);
        if (!library) { report("load_inner_failed reason=%s", dlerror()); return; }
        auto get = reinterpret_cast<OfxPlugin* (*)(int)>(dlsym(library, "OfxGetPlugin"));
        if (get) inner = get(0);
        report("candidate_loaded wrapper=%s inner=%s plugin=%s notarized=0 host_acceptance=pending",
               module.dli_fname, path.c_str(), inner ? inner->pluginIdentifier : "missing");
    });
    report("get_plugin index=%d result=%p outer_id=%s api_version=%d ofx_version=%u.%u", index,
           inner ? &plugin : nullptr, plugin.pluginIdentifier, plugin.apiVersion, plugin.pluginVersionMajor, plugin.pluginVersionMinor);
    return inner ? &plugin : nullptr;
}
