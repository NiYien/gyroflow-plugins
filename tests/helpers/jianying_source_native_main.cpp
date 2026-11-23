// SPDX-License-Identifier: GPL-3.0-or-later
#include <iostream>
#include <stdexcept>
#ifdef _WIN32
#include "jianying_source_path.h"
#else
#include "macos/source_path.h"
#include <dlfcn.h>
#endif
int main(int argc, char** argv) {
    try {
        if (argc != 2) throw std::runtime_error("Expected fixture library path");
#ifdef _WIN32
        auto library = LoadLibraryA(argv[1]);
        auto raw_get = GetProcAddress(library, "fixture_effect");
        auto raw_pending = GetProcAddress(library, "fixture_pending_effect");
        auto raw_attach = GetProcAddress(library, "fixture_attach_message");
        void* (*get)(const char*) = nullptr;
        void* (*get_pending)(const char*) = nullptr;
        void (*attach_message)() = nullptr;
        static_assert(sizeof(get) == sizeof(raw_get));
        std::memcpy(&get, &raw_get, sizeof(get));
        std::memcpy(&get_pending, &raw_pending, sizeof(get_pending));
        std::memcpy(&attach_message, &raw_attach, sizeof(attach_message));
        const std::string expected = "C:/owned/media/clip.mov";
#else
        auto library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
        auto get = library ? reinterpret_cast<void* (*)(const char*)>(dlsym(library, "fixture_effect")) : nullptr;
        const std::string expected = "/owned/media/clip.mov";
#endif
        if (!get) throw std::runtime_error("Could not load source fixture");
#ifdef _WIN32
        if (!get_pending || !attach_message) throw std::runtime_error("Missing pending fixture entrypoints");
        const auto pending = get_pending(expected.c_str());
        const auto before = jianying_source_path::inspect_source(pending);
        if (before.status != jianying_source_path::Status::MissingMessageCallback) {
            throw std::runtime_error(std::string("Expected pending message callback, got ") + jianying_source_path::status_name(before.status));
        }
        attach_message();
        const auto after = jianying_source_path::inspect_source(pending);
        if (after.status != jianying_source_path::Status::Ready || after.source != expected) {
            throw std::runtime_error(std::string("Native MSVC source contract failed after message attach: ") + jianying_source_path::status_name(after.status));
        }
        const auto effect = get(expected.c_str());
        std::string source;
        if (!jianying_source_path::copy_source_path(effect, source) || source != expected) {
            const auto obs = jianying_source_path::inspect_source(effect);
            throw std::runtime_error(std::string("Native MSVC source contract rejected fixture: ") + jianying_source_path::status_name(obs.status));
        }
#else
        const auto effect = get(expected.c_str());
        if (!jianying_macos_source_path::module_supported()) throw std::runtime_error("Native module rejected fixture");
        const auto observation = jianying_macos_source_path::inspect_source(effect);
        if (observation.status != jianying_macos_source_path::Status::Ready || observation.source != expected)
            throw std::runtime_error(jianying_macos_source_path::status_name(observation.status));
#endif
        std::cout << "{\"passed\":true,\"native_module\":true,\"compiler_rtti\":true,\"real_editor_loaded\":false}\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
