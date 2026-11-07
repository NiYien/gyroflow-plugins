// SPDX-License-Identifier: GPL-3.0-or-later
// This host provides typed parameter storage without creating a Metal device or image.
#include <ofxCore.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#include <cstdarg>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <filesystem>
#include <iostream>
#include <map>
#include <stdexcept>
#include <string>
#include <variant>
#include <vector>

namespace {
using Value = std::variant<int, double, std::string, void*>;
struct Properties { std::map<std::string, std::vector<Value>> values; };
struct Parameter { std::string name, expected_type, actual_type; Properties props; Value value; int value_status = kOfxStatOK; };
struct Effect { std::map<std::string, Parameter> parameters; };
OfxPropertySuiteV1 property_suite{};
OfxImageEffectSuiteV1 image_suite{};
OfxParameterSuiteV1 parameter_suite{};
OfxPlugin* wrapper = nullptr;
Effect* active_effect = nullptr;
unsigned parameter_sets = 0, parameter_handles = 0, parameter_properties_reads = 0;
unsigned value_reads = 0, wrong_type_reads = 0, value_writes = 0, property_writes = 0, image_calls = 0;
std::map<std::string, unsigned> reads;
bool reentrant = false, reentered = false;
int nested_status = -1;
void require(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
OfxPropertySetHandle handle(Properties& props) { return reinterpret_cast<OfxPropertySetHandle>(&props); }
Properties& props(OfxPropertySetHandle value) { return *reinterpret_cast<Properties*>(value); }
template<class T> OfxStatus get(OfxPropertySetHandle value, const char* key, int index, T* output) {
    if (!value || !output || index < 0) return kOfxStatErrBadHandle;
    const auto found = props(value).values.find(key);
    if (found == props(value).values.end()) return kOfxStatErrUnknown;
    if (static_cast<std::size_t>(index) >= found->second.size()) return kOfxStatErrBadIndex;
    const auto* typed = std::get_if<T>(&found->second[index]);
    if (!typed) return kOfxStatErrValue;
    *output = *typed;
    return kOfxStatOK;
}
OfxStatus get_string(OfxPropertySetHandle value, const char* key, int index, const char** output) {
    if (!value || !output || index < 0) return kOfxStatErrBadHandle;
    const auto found = props(value).values.find(key);
    if (found == props(value).values.end()) return kOfxStatErrUnknown;
    if (static_cast<std::size_t>(index) >= found->second.size()) return kOfxStatErrBadIndex;
    const auto* typed = std::get_if<std::string>(&found->second[index]);
    if (!typed) return kOfxStatErrValue;
    *output = typed->c_str();
    return kOfxStatOK;
}
OfxStatus dimension(OfxPropertySetHandle value, const char* key, int* output) {
    if (!value || !output) return kOfxStatErrBadHandle;
    const auto found = props(value).values.find(key);
    if (found == props(value).values.end()) return kOfxStatErrUnknown;
    *output = static_cast<int>(found->second.size());
    return kOfxStatOK;
}
template<class T> OfxStatus get_n(OfxPropertySetHandle value, const char* key, int count, T* output) {
    for (int i = 0; i < count; ++i) { const auto status = get(value, key, i, &output[i]); if (status != kOfxStatOK) return status; }
    return kOfxStatOK;
}
template<class T> OfxStatus write_property(OfxPropertySetHandle, const char*, int, T) { ++property_writes; return kOfxStatOK; }
template<class T> OfxStatus write_properties(OfxPropertySetHandle, const char*, int, const T*) { ++property_writes; return kOfxStatOK; }
OfxStatus reset_property(OfxPropertySetHandle, const char*) { ++property_writes; return kOfxStatOK; }
OfxStatus get_param_set(OfxImageEffectHandle effect, OfxParamSetHandle* output) {
    ++parameter_sets;
    *output = reinterpret_cast<OfxParamSetHandle>(effect); return kOfxStatOK;
}
OfxStatus get_parameter(OfxParamSetHandle raw, const char* name, OfxParamHandle* output, OfxPropertySetHandle* properties) {
    ++parameter_handles;
    auto& parameters = reinterpret_cast<Effect*>(raw)->parameters;
    const auto found = parameters.find(name);
    if (found == parameters.end()) return kOfxStatErrUnknown;
    *output = reinterpret_cast<OfxParamHandle>(&found->second);
    if (properties) *properties = handle(found->second.props);
    return kOfxStatOK;
}
OfxStatus parameter_properties(OfxParamHandle raw, OfxPropertySetHandle* output) {
    ++parameter_properties_reads;
    *output = handle(reinterpret_cast<Parameter*>(raw)->props); return kOfxStatOK;
}
OfxStatus get_value(OfxParamHandle raw, ...) {
    auto& parameter = *reinterpret_cast<Parameter*>(raw);
    ++value_reads;
    ++reads[parameter.name];
    if (parameter.expected_type != parameter.actual_type) { ++wrong_type_reads; return kOfxStatErrValue; }
    if (parameter.value_status != kOfxStatOK) return parameter.value_status;
    if (reentrant && !reentered && parameter.name == "ProjectPath") {
        reentered = true;
        nested_status = wrapper->mainEntry(kOfxActionEndInstanceChanged, active_effect, nullptr, nullptr);
    }
    va_list arguments;
    va_start(arguments, raw);
    if (parameter.expected_type == kOfxParamTypeString)
        *va_arg(arguments, char**) = const_cast<char*>(std::get<std::string>(parameter.value).c_str());
    else if (parameter.expected_type == kOfxParamTypeDouble)
        *va_arg(arguments, double*) = std::get<double>(parameter.value);
    else *va_arg(arguments, int*) = std::get<int>(parameter.value);
    va_end(arguments);
    return kOfxStatOK;
}
OfxStatus write_value(OfxParamHandle, ...) { ++value_writes; return kOfxStatOK; }
OfxStatus write_value_at_time(OfxParamHandle, OfxTime, ...) { ++value_writes; return kOfxStatOK; }
OfxStatus get_image(OfxImageClipHandle, OfxTime, const OfxRectD*, OfxPropertySetHandle*) { ++image_calls; return kOfxStatFailed; }
const void* fetch_suite(OfxPropertySetHandle, const char* name, int version) {
    if (version != 1) return nullptr;
    if (!std::strcmp(name, kOfxPropertySuite)) return &property_suite;
    if (!std::strcmp(name, kOfxImageEffectSuite)) return &image_suite;
    if (!std::strcmp(name, kOfxParameterSuite)) return &parameter_suite;
    return nullptr;
}
void initialize_suites() {
    property_suite.propGetInt = get<int>; property_suite.propGetDouble = get<double>; property_suite.propGetString = get_string;
    property_suite.propGetPointer = get<void*>; property_suite.propGetIntN = get_n<int>; property_suite.propGetPointerN = get_n<void*>;
    property_suite.propGetDimension = dimension;
    property_suite.propSetInt = write_property<int>; property_suite.propSetDouble = write_property<double>;
    property_suite.propSetPointer = write_property<void*>; property_suite.propSetString = write_property<const char*>;
    property_suite.propSetIntN = write_properties<int>; property_suite.propSetDoubleN = write_properties<double>;
    property_suite.propSetPointerN = write_properties<void*>; property_suite.propSetStringN = write_properties<const char*>;
    property_suite.propReset = reset_property;
    image_suite.getParamSet = get_param_set; image_suite.clipGetImage = get_image;
    parameter_suite.paramGetHandle = get_parameter; parameter_suite.paramGetPropertySet = parameter_properties;
    parameter_suite.paramGetValue = get_value; parameter_suite.paramSetValue = write_value;
    parameter_suite.paramSetValueAtTime = write_value_at_time;
}
void add(Effect& effect, const char* name, const char* type, Value value) {
    Parameter parameter{name, type, type, {}, std::move(value)};
    parameter.props.values[kOfxParamPropType] = {std::string(type)};
    effect.parameters.emplace(name, std::move(parameter));
}
}

int main(int argc, char** argv) {
    try {
        require(argc == 4, "Usage: test-snapshot wrapper.ofx log scenario");
        require(std::filesystem::path(argv[1]).is_absolute() && std::filesystem::path(argv[2]).is_absolute(), "Absolute paths required");
        require(setenv("NIYIEN_JIANYING_MAC_LOG", argv[2], 1) == 0, "Cannot isolate snapshot log");
        require(setenv("NIYIEN_JIANYING_MAC_OBSERVE_UI", "1", 1) == 0, "Cannot test obsolete observation switch");
        initialize_suites();
        void* library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
        if (!library) { const char* error = dlerror(); throw std::runtime_error(error ? error : "Wrapper dlopen failed"); }
        auto get_plugin = reinterpret_cast<OfxPlugin* (*)(int)>(dlsym(library, "OfxGetPlugin"));
        require(get_plugin != nullptr, "Wrapper export missing");
        wrapper = get_plugin(0);
        require(wrapper != nullptr, "Mock inner not found");
        void* inner = dlopen((std::filesystem::path(argv[1]).parent_path() / "libgyroflow_ofx.dylib").c_str(), RTLD_NOW | RTLD_LOCAL);
        auto calls = reinterpret_cast<unsigned (*)()>(dlsym(inner, "MockSnapshotCalls"));
        require(calls != nullptr, "Mock inner counter missing");
        Properties host_properties;
        host_properties.values[kOfxPropName] = {std::string("com.lemon.lvpro")};
        OfxHost host{handle(host_properties), fetch_suite};
        wrapper->setHost(&host);
        Effect effect;
        active_effect = &effect;
        add(effect, "ProjectPath", kOfxParamTypeString, std::string("fixture folder/\"quoted\"\\file\nclip.gyroflow"));
        add(effect, "Smoothness", kOfxParamTypeDouble, 48.0);
        add(effect, "ZoomMode", kOfxParamTypeChoice, 2);
        add(effect, "TranslationEnabled", kOfxParamTypeBoolean, 0);
        add(effect, "InstanceId", kOfxParamTypeString, std::string("instance-\"one\"\\tail"));
        const std::string scenario = argv[3];
        auto invoke = [&](const char* action, Properties* in = nullptr) {
            return wrapper->mainEntry(action, &effect, in ? handle(*in) : nullptr, nullptr);
        };
        int status = -1;
        if (scenario == "end") status = invoke(kOfxActionEndInstanceChanged);
        else if (scenario == "sync") {
            effect.parameters.at("Smoothness").value = 48.5;
            effect.parameters.at("ZoomMode").value = 1;
            effect.parameters.at("TranslationEnabled").value = 1;
            status = invoke(kOfxActionSyncPrivateData);
        } else if (scenario == "other") {
            for (const auto* action : {kOfxActionCreateInstance, kOfxActionBeginInstanceChanged, kOfxActionInstanceChanged})
                require(invoke(action) == kOfxStatOK, "Non-snapshot action changed status");
            status = kOfxStatOK;
        } else if (scenario == "inner-failure") {
            Properties in;
            in.values["MockInnerStatus"] = {kOfxStatFailed};
            status = invoke(kOfxActionEndInstanceChanged, &in);
            require(status == kOfxStatFailed, "Inner failure status changed");
        } else if (scenario == "missing") {
            effect.parameters.erase("TranslationEnabled");
            effect.parameters.at("Smoothness").value_status = kOfxStatFailed;
            status = invoke(kOfxActionEndInstanceChanged);
        } else if (scenario == "type-mismatch") {
            auto& parameter = effect.parameters.at("Smoothness");
            parameter.actual_type = kOfxParamTypeString;
            parameter.props.values[kOfxParamPropType] = {std::string(kOfxParamTypeString)};
            status = invoke(kOfxActionEndInstanceChanged);
        } else if (scenario == "render") {
            Properties in;
            in.values["OfxImageEffectPropMetalEnabled"] = {1};
            in.values["OfxImageEffectPropOpenGLEnabled"] = {0};
            in.values["OfxImageEffectPropMetalCommandQueue"] = {reinterpret_cast<void*>(0x1)};
            status = invoke(kOfxImageEffectActionRender, &in);
        } else if (scenario == "reentrant") {
            reentrant = true;
            status = invoke(kOfxActionEndInstanceChanged);
            require(!reentered && calls() == 1, "A diagnostic getter unexpectedly triggered reentry");
        } else throw std::runtime_error("Unknown snapshot scenario");
        const unsigned expected_reads = 0;
        require(status == (scenario == "inner-failure" ? kOfxStatFailed : kOfxStatOK), "Snapshot changed action status");
        require(parameter_sets == 0 && parameter_handles == 0 && parameter_properties_reads == 0, "Adapter queried diagnostic parameters");
        require(value_reads == expected_reads && wrong_type_reads == 0, "Unexpected typed getter count");
        require(value_writes == 0 && property_writes == 0 && image_calls == 0, "Snapshot changed host state or acquired an image");
        if (scenario == "type-mismatch") require(reads["Smoothness"] == 0, "Type mismatch must not call typed getter");
        std::cout << "{\"scenario\":\"" << scenario << "\",\"status\":" << status << ",\"typed_reads\":" << value_reads
                  << ",\"wrong_type_reads\":" << wrong_type_reads << ",\"value_writes\":" << value_writes
                  << ",\"property_writes\":" << property_writes << ",\"image_calls\":" << image_calls
                  << ",\"inner_calls\":" << calls() << ",\"nested_status\":" << nested_status << "}\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << "FAIL: " << error.what() << '\n'; return 1; }
}
