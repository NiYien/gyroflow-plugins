// SPDX-License-Identifier: GPL-3.0-or-later
// Exercise the real forwarding dylib with public OFX suites and owned Metal test resources.
#include <ofxCore.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <cstdarg>
#include <cstring>
#include <cstdlib>
#include <dlfcn.h>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <stdexcept>
#include <string>
#include <variant>
#include <vector>

namespace {
using Value = std::variant<int, double, std::string, void*>;
struct Properties { std::map<std::string, std::vector<Value>> values; };
struct Parameter { Properties props; std::string type; };
struct Clip { Properties props; };
struct Effect { Properties props; std::map<std::string, Parameter> parameters; Clip source, output; };
struct Write { std::string type; bool timed; double time; std::vector<Value> values; };
OfxPropertySuiteV1 property_suite{};
OfxImageEffectSuiteV1 image_suite{};
OfxParameterSuiteV1 parameter_suite{};
std::vector<Write> writes;
unsigned property_writes = 0, fetched = 0, released = 0, n_setter_failure_calls = 0;
void require(bool value, const std::string& message) { if (!value) throw std::runtime_error(message); }
OfxPropertySetHandle handle(Properties& value) { return reinterpret_cast<OfxPropertySetHandle>(&value); }
Properties& properties(OfxPropertySetHandle value) { return *reinterpret_cast<Properties*>(value); }
template<class T> OfxStatus set(OfxPropertySetHandle p, const char* key, int index, T value) {
    if (!p || index < 0) return kOfxStatErrBadHandle;
    ++property_writes;
    auto& values = properties(p).values[key];
    if (values.size() <= static_cast<std::size_t>(index)) values.resize(index + 1);
    values[index] = value;
    return kOfxStatOK;
}
OfxStatus set_string(OfxPropertySetHandle p, const char* key, int index, const char* value) {
    return set(p, key, index, std::string(value));
}
OfxStatus set_strings(OfxPropertySetHandle p, const char* key, int count, const char* const* values) {
    if (!std::strcmp(key, "MockNFailure")) { ++n_setter_failure_calls; return kOfxStatErrUnknown; }
    for (int i = 0; i < count; ++i) set_string(p, key, i, values[i]);
    return kOfxStatOK;
}
template<class T> OfxStatus get(OfxPropertySetHandle p, const char* key, int index, T* value) {
    if (!p || !value || index < 0) return kOfxStatErrBadHandle;
    const auto found = properties(p).values.find(key);
    if (found == properties(p).values.end()) return kOfxStatErrUnknown;
    if (static_cast<std::size_t>(index) >= found->second.size()) return kOfxStatErrBadIndex;
    const auto* result = std::get_if<T>(&found->second[index]);
    if (!result) return kOfxStatErrValue;
    *value = *result;
    return kOfxStatOK;
}
OfxStatus get_string(OfxPropertySetHandle p, const char* key, int index, const char** value) {
    if (!p || !value || index < 0) return kOfxStatErrBadHandle;
    const auto found = properties(p).values.find(key);
    if (found == properties(p).values.end()) return kOfxStatErrUnknown;
    if (static_cast<std::size_t>(index) >= found->second.size()) return kOfxStatErrBadIndex;
    const auto* result = std::get_if<std::string>(&found->second[index]);
    if (!result) return kOfxStatErrValue;
    *value = result->c_str();
    return kOfxStatOK;
}
template<class T> OfxStatus get_n(OfxPropertySetHandle p, const char* key, int count, T* values) {
    for (int i = 0; i < count; ++i) { const auto status = get(p, key, i, &values[i]); if (status != kOfxStatOK) return status; }
    return kOfxStatOK;
}
OfxStatus dimension(OfxPropertySetHandle p, const char* key, int* count) {
    if (!p || !count) return kOfxStatErrBadHandle;
    const auto found = properties(p).values.find(key);
    if (found == properties(p).values.end()) return kOfxStatErrUnknown;
    *count = static_cast<int>(found->second.size());
    return kOfxStatOK;
}
OfxStatus get_effect_properties(OfxImageEffectHandle effect, OfxPropertySetHandle* value) {
    *value = handle(reinterpret_cast<Effect*>(effect)->props); return kOfxStatOK;
}
OfxStatus get_parameters(OfxImageEffectHandle effect, OfxParamSetHandle* value) {
    *value = reinterpret_cast<OfxParamSetHandle>(effect); return kOfxStatOK;
}
OfxStatus define_parameter(OfxParamSetHandle set, const char* type, const char* name, OfxPropertySetHandle* value) {
    auto& parameter = reinterpret_cast<Effect*>(set)->parameters[name];
    parameter.type = type;
    parameter.props.values[kOfxParamPropType] = {std::string(type)};
    *value = handle(parameter.props);
    return kOfxStatOK;
}
OfxStatus get_parameter(OfxParamSetHandle set, const char* name, OfxParamHandle* value, OfxPropertySetHandle* props) {
    auto& parameters = reinterpret_cast<Effect*>(set)->parameters;
    const auto found = parameters.find(name);
    if (found == parameters.end()) return kOfxStatErrUnknown;
    *value = reinterpret_cast<OfxParamHandle>(&found->second);
    if (props) *props = handle(found->second.props);
    return kOfxStatOK;
}
OfxStatus parameter_properties(OfxParamHandle parameter, OfxPropertySetHandle* value) {
    *value = handle(reinterpret_cast<Parameter*>(parameter)->props); return kOfxStatOK;
}
OfxStatus record_write(OfxParamHandle raw, bool timed, double time, va_list& args) {
    const auto& type = reinterpret_cast<Parameter*>(raw)->type;
    Write write{type, timed, time, {}};
    if (type == kOfxParamTypeString || type == kOfxParamTypeCustom) write.values.emplace_back(std::string(va_arg(args, const char*)));
    else {
        const bool integers = type == kOfxParamTypeInteger || type == kOfxParamTypeBoolean || type == kOfxParamTypeChoice
            || type == kOfxParamTypePushButton || type == kOfxParamTypeInteger2D || type == kOfxParamTypeInteger3D;
        const int count = type == kOfxParamTypeDouble2D || type == kOfxParamTypeInteger2D ? 2 :
            type == kOfxParamTypeDouble3D || type == kOfxParamTypeInteger3D || type == kOfxParamTypeRGB ? 3 :
            type == kOfxParamTypeRGBA ? 4 : 1;
        for (int i = 0; i < count; ++i) {
            if (integers) write.values.emplace_back(va_arg(args, int));
            else write.values.emplace_back(va_arg(args, double));
        }
    }
    writes.push_back(std::move(write));
    return kOfxStatOK;
}
OfxStatus set_value(OfxParamHandle parameter, ...) {
    va_list args; va_start(args, parameter); const auto result = record_write(parameter, false, 0, args); va_end(args); return result;
}
OfxStatus set_value_at_time(OfxParamHandle parameter, OfxTime time, ...) {
    va_list args; va_start(args, time); const auto result = record_write(parameter, true, time, args); va_end(args); return result;
}
OfxStatus get_clip(OfxImageEffectHandle raw, const char* name, OfxImageClipHandle* value, OfxPropertySetHandle* props) {
    auto* effect = reinterpret_cast<Effect*>(raw);
    Clip* clip = !std::strcmp(name, "Source") ? &effect->source : &effect->output;
    *value = reinterpret_cast<OfxImageClipHandle>(clip);
    if (props) *props = handle(clip->props);
    return kOfxStatOK;
}
OfxStatus get_image(OfxImageClipHandle raw, OfxTime, const OfxRectD*, OfxPropertySetHandle* value) {
    ++fetched; *value = handle(reinterpret_cast<Clip*>(raw)->props); return kOfxStatOK;
}
OfxStatus release_image(OfxPropertySetHandle) { ++released; return kOfxStatOK; }
const void* fetch_suite(OfxPropertySetHandle, const char* name, int version) {
    if (version != 1) return nullptr;
    if (!std::strcmp(name, kOfxPropertySuite)) return &property_suite;
    if (!std::strcmp(name, kOfxImageEffectSuite)) return &image_suite;
    if (!std::strcmp(name, kOfxParameterSuite)) return &parameter_suite;
    return nullptr;
}
void initialize_suites() {
    property_suite.propGetInt = get<int>; property_suite.propGetDouble = get<double>; property_suite.propGetPointer = get<void*>;
    property_suite.propGetIntN = get_n<int>; property_suite.propGetPointerN = get_n<void*>; property_suite.propGetString = get_string;
    property_suite.propGetDimension = dimension; property_suite.propSetInt = set<int>; property_suite.propSetPointer = set<void*>;
    property_suite.propSetString = set_string; property_suite.propSetStringN = set_strings;
    image_suite.getPropertySet = get_effect_properties; image_suite.getParamSet = get_parameters;
    image_suite.clipGetHandle = get_clip; image_suite.clipGetImage = get_image; image_suite.clipReleaseImage = release_image;
    parameter_suite.paramGetPropertySet = parameter_properties; parameter_suite.paramDefine = define_parameter;
    parameter_suite.paramGetHandle = get_parameter; parameter_suite.paramSetValue = set_value; parameter_suite.paramSetValueAtTime = set_value_at_time;
}
}

int main(int argc, char** argv) {
    try { @autoreleasepool {
        require(argc == 3, "Usage: test-adapter /absolute/path/to/wrapper.ofx /absolute/path/to/log");
        require(std::filesystem::path(argv[1]).is_absolute() && std::filesystem::path(argv[2]).is_absolute(), "Absolute paths required");
        require(setenv("NIYIEN_JIANYING_MAC_LOG", argv[2], 1) == 0, "Cannot isolate test log");
        initialize_suites();
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        require(device != nil, "Real Metal device unavailable; rerun with full device permissions");
        id<MTLCommandQueue> queue = [device newCommandQueue];
        id<MTLBuffer> source = [device newBufferWithLength:128 options:MTLResourceStorageModeShared];
        id<MTLBuffer> destination = [device newBufferWithLength:128 options:MTLResourceStorageModeShared];
        require(queue && source && destination, "Metal test resource allocation");
        auto* bytes = static_cast<unsigned char*>(source.contents);
        for (unsigned i = 0; i < 128; ++i) bytes[i] = static_cast<unsigned char>((i * 17) ^ 0x5a);
        std::memset(destination.contents, 0, 128);
        void* library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
        if (!library) { const char* error = dlerror(); throw std::runtime_error(error ? error : "Wrapper dlopen"); }
        auto get_plugin = reinterpret_cast<OfxPlugin* (*)(int)>(dlsym(library, "OfxGetPlugin"));
        auto count = reinterpret_cast<int (*)()>(dlsym(library, "OfxGetNumberOfPlugins"));
        require(get_plugin && count && count() == 1 && !get_plugin(1), "Wrapper exports and index bounds");
        OfxPlugin* plugin = get_plugin(0);
        require(plugin && std::string(plugin->pluginIdentifier) == "xyz.niyien.gyroflow.jianying.batch11"
            && plugin->pluginVersionMajor == 1 && plugin->pluginVersionMinor == 11, "Draft identity and OFX version");
        const auto inner_path = std::filesystem::path(argv[1]).parent_path() / "libgyroflow_ofx.dylib";
        void* inner_library = dlopen(inner_path.c_str(), RTLD_NOW | RTLD_LOCAL);
        require(inner_library, "Mock inner loaded from same directory");
        auto render_calls = reinterpret_cast<unsigned (*)()>(dlsym(inner_library, "MockRenderCalls"));
        auto action_calls = reinterpret_cast<unsigned (*)()>(dlsym(inner_library, "MockActionCalls"));
        require(render_calls && action_calls, "Mock test counters");
        Properties host_properties;
        host_properties.values[kOfxPropName] = {std::string("com.lemon.lvpro")};
        OfxHost host{handle(host_properties), fetch_suite};
        plugin->setHost(&host);
        int missing = 0;
        require(get(handle(host_properties), "NiYienJianyingMacMetalAdapter", 0, &missing) == kOfxStatErrUnknown,
                "Original host must not contain private adapter properties");
        const auto before_private_writes = property_writes;
        require(plugin->mainEntry(kOfxActionLoad, nullptr, nullptr, nullptr) == kOfxStatOK
            && property_writes == before_private_writes, "Proxy private properties are read-only and never mutate host");
        Effect effect;
        for (const auto& item : std::vector<std::pair<const char*, const char*>>{
            {"Integer", kOfxParamTypeInteger}, {"Boolean", kOfxParamTypeBoolean}, {"Choice", kOfxParamTypeChoice},
            {"Button", kOfxParamTypePushButton}, {"Double", kOfxParamTypeDouble}, {"String", kOfxParamTypeString},
            {"Custom", kOfxParamTypeCustom}, {"Integer2D", kOfxParamTypeInteger2D}, {"Integer3D", kOfxParamTypeInteger3D},
            {"Double2D", kOfxParamTypeDouble2D}, {"Double3D", kOfxParamTypeDouble3D}, {"RGB", kOfxParamTypeRGB},
            {"RGBA", kOfxParamTypeRGBA}, {"Unknown", "UnknownTestType"}, {"Main", kOfxParamTypePage}}) {
            OfxPropertySetHandle ignored = nullptr;
            define_parameter(reinterpret_cast<OfxParamSetHandle>(&effect), item.second, item.first, &ignored);
        }
        auto invoke = [&](const char* action, Properties* in = nullptr) {
            return plugin->mainEntry(action, &effect, in ? handle(*in) : nullptr, nullptr);
        };
        const auto before_context_actions = action_calls(), before_context_renders = render_calls();
        const auto before_context_properties = property_writes, before_context_fetched = fetched, before_context_released = released;
        const auto before_context_values = writes.size();
        for (const auto* action : {"OfxActionMetalContextAttached", "OfxActionMetalContextDetached"}) {
            require(invoke(action) == kOfxStatReplyDefault && action_calls() == before_context_actions
                && render_calls() == before_context_renders && property_writes == before_context_properties
                && writes.size() == before_context_values && fetched == before_context_fetched && released == before_context_released,
                "Metal context no-op must not enter inner or mutate host");
        }
        const unsigned char zero[128]{};
        require(!std::memcmp(destination.contents, zero, sizeof(zero)), "Metal context no-op must not touch GPU destination");
        require(invoke("MockUnknownAction") == kOfxStatReplyYes && action_calls() == before_context_actions + 1,
                "Unknown non-Metal action must still forward the original inner status");
        require(invoke(kOfxActionDescribe) == kOfxStatOK, "Describe forwarding");
        const char* label = nullptr;
        require(get_string(handle(effect.props), kOfxPropLabel, 0, &label) == kOfxStatOK && std::string(label) == "Gyroflow", "Product display label");
        require(invoke(kOfxImageEffectActionDescribeInContext) == kOfxStatOK, "InContext forwarding");
        auto& diagnostic = effect.parameters.at("DiagnosticMode");
        require(diagnostic.type == kOfxParamTypeChoice, "DiagnosticMode identity");
        int hidden = 0;
        require(get(handle(diagnostic.props), kOfxParamPropSecret, 0, &hidden) == kOfxStatOK && hidden == 1, "DiagnosticMode stays hidden");
        require(invoke("MockVarargs") == kOfxStatOK && writes.size() == 26, "Public varargs scalar/vector/string forwarding");
        for (std::size_t i = 0; i < writes.size(); i += 2) {
            require(!writes[i].timed && writes[i + 1].timed && writes[i + 1].time == 7.5
                && writes[i].values == writes[i + 1].values, "AtTime values or time changed");
        }
        require(writes[0].values == std::vector<Value>{17}, "Integer payload");
        require(writes[8].values == std::vector<Value>{2.25}, "Double payload");
        require(writes[10].values == std::vector<Value>{std::string("hello")}, "String payload");
        require(writes[14].values == std::vector<Value>{-2, 9}, "Integer2D payload");
        require(writes[18].values == std::vector<Value>{0.25, 0.75}, "Double2D payload");
        require(writes[24].values == std::vector<Value>{0.1, 0.2, 0.3, 0.4}, "RGBA payload");
        const auto before_n_failure_writes = property_writes;
        require(invoke("MockNSetterError") == kOfxStatErrUnknown && n_setter_failure_calls == 1,
                "Non-Render N setter status must propagate without scalar fallback");
        require(invoke("MockNSetterMissing") == kOfxStatErrMissingHostFeature
            && property_writes == before_n_failure_writes,
            "Missing N setter must report MissingHostFeature without mutating host");
        for (auto* clip : {&effect.source, &effect.output}) {
            clip->props.values[kOfxImagePropBounds] = {0, 0, 8, 4};
            clip->props.values[kOfxImagePropRowBytes] = {32};
            clip->props.values[kOfxImageEffectPropPixelDepth] = {std::string(kOfxBitDepthByte)};
            clip->props.values[kOfxImageEffectPropComponents] = {std::string(kOfxImageComponentRGBA)};
        }
        effect.source.props.values[kOfxImagePropData] = {(__bridge void*)source};
        effect.output.props.values[kOfxImagePropData] = {(__bridge void*)destination};
        Properties render;
        render.values["OfxImageEffectPropMetalEnabled"] = {1};
        render.values["OfxImageEffectPropOpenGLEnabled"] = {0};
        render.values["OfxImageEffectPropMetalCommandQueue"] = {(__bridge void*)queue};
        const auto calls = render_calls();
        Properties invalid = render;
        invalid.values["OfxImageEffectPropMetalEnabled"] = {0};
        require(invoke(kOfxImageEffectActionRender, &invalid) == kOfxStatErrUnsupported && render_calls() == calls,
                "Non-Metal Render must not enter inner");
        invalid = render;
        invalid.values["OfxImageEffectPropMetalTextureEnabled"] = {1};
        require(invoke(kOfxImageEffectActionRender, &invalid) == kOfxStatErrUnsupported && render_calls() == calls,
                "Texture conflict must not enter inner");
        const auto before_writes = writes.size();
        const auto before_property_writes = property_writes;
        render.values["MockWriteInRender"] = {1};
        require(invoke(kOfxImageEffectActionRender, &render) == kOfxStatFailed && writes.size() == before_writes,
                "Render value setters must be blocked before reaching host");
        int syncing = 0;
        require(property_writes == before_property_writes + 1
            && get(handle(effect.props), "OfxPropParamSetNeedsSyncing", 0, &syncing) == kOfxStatOK && syncing == 1,
            "Render property writes are blocked except the explicit needs-sync signal");
        render.values.erase("MockWriteInRender");
        require(invoke(kOfxImageEffectActionRender, &render) == kOfxStatOK && fetched == 2 && released == 2,
                "Real Metal buffer get/release forwarding");
        require(!std::memcmp(source.contents, destination.contents, 128), "GPU passthrough callback output");
        effect.source.props.values[kOfxImagePropRowBytes] = {-32};
        require(invoke(kOfxImageEffectActionRender, &render) == kOfxStatErrUnsupported && fetched == 3 && released == 3,
                "Rejected image must be released exactly once");
        require(get(handle(host_properties), "NiYienJianyingMacMetalAdapter", 0, &missing) == kOfxStatErrUnknown,
                "Private property synthesis must not mutate original host");
        std::ifstream log_file(argv[2]);
        const std::string log(std::istreambuf_iterator<char>(log_file), {});
        for (const auto& expected : {"set_host_begin", "set_host_suites", "set_host_property_functions",
            "set_host_end host_name_status=0 host_name=com.lemon.lvpro", "suite_fetch name=OfxPropertySuite",
            "private_get_int property=NiYienJianyingMacMetalAdapter", "private_get_pointer property=NiYienJianyingMacMetalCopyV1",
            "action_begin action=MockNSetterError", "action_inner_return action=MockNSetterError", "action_end action=MockNSetterError"})
            require(log.find(expected) != std::string::npos, "Missing diagnostic log field: " + std::string(expected));
        require(log.find("property_write_result action=MockNSetterError operation=propSetStringN property=MockNFailure status="
            + std::to_string(kOfxStatErrUnknown)) != std::string::npos, "N setter original failure must be logged");
        require(log.find("property_write_result action=MockNSetterMissing operation=propSetDoubleN property=MockNMissing status="
            + std::to_string(kOfxStatErrMissingHostFeature)) != std::string::npos, "N setter missing function must be logged");
        for (const auto* action : {"OfxActionMetalContextAttached", "OfxActionMetalContextDetached"})
            require(log.find(std::string("metal_context_noop action=") + action + " status=14 forwarded=0 gpu_commands=0 parameter_writes=0")
                != std::string::npos, "Metal context no-op diagnostic must identify bypassed inner");
        std::cout << "PASS: mock-only OFX forwarding ABI, private properties, 26 typed value calls, Render write guard, real Metal validation/copy and image release\n";
        std::cout << "{\"scope\":\"native mock only; no Jianying host acceptance\",\"vararg_host_calls\":" << writes.size()
                  << ",\"images_fetched\":" << fetched << ",\"images_released\":" << released << ",\"gpu_copy_bytes\":128,\"n_setter_errors_preserved\":true,\"diagnostic_fields_verified\":true,\"metal_context_noops\":2,\"unknown_nonmetal_forwarded\":true}\n";
        return 0;
    }} catch (const std::exception& error) { std::cerr << "FAIL: " << error.what() << '\n'; return 1; }
}
