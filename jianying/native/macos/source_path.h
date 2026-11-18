// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "source_layout.h"
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#include <dlfcn.h>
#include <map>
#include <memory>
#include <mutex>

namespace jianying_macos_source_path {
using jianying_source_contract::Status;
using jianying_source_contract::Observation;
using jianying_source_contract::status_name;
namespace detail {
using namespace jianying_source_contract;
inline bool mach_read(std::uintptr_t address, void* output, std::size_t size, void*) {
    mach_vm_size_t copied = 0;
    return mach_vm_read_overwrite(mach_task_self(), static_cast<mach_vm_address_t>(address),
        static_cast<mach_vm_size_t>(size), reinterpret_cast<mach_vm_address_t>(output), &copied) == KERN_SUCCESS && copied == size;
}
inline bool slid_address(std::intptr_t slide, std::uint64_t address, std::uintptr_t& result) {
    if (slide >= 0) return safe_add(static_cast<std::uintptr_t>(slide), address, result);
    const auto magnitude = static_cast<std::uint64_t>(-(slide + 1)) + 1;
    if (address < magnitude) return false;
    result = static_cast<std::uintptr_t>(address - magnitude);
    return true;
}
inline Module inspect_module(std::uintptr_t base) {
    const Reader reader{mach_read};
    mach_header_64 header{};
    if (!reader.read(base, &header, sizeof(header)) || header.magic != MH_MAGIC_64
        || header.cputype != CPU_TYPE_ARM64 || header.cpusubtype != CPU_SUBTYPE_ARM64_ALL
        || header.ncmds > 256 || header.sizeofcmds > 1024 * 1024) return {};
    bool found = false;
    std::intptr_t slide = 0;
    for (std::uint32_t i = 0; i < _dyld_image_count(); ++i) {
        if (reinterpret_cast<std::uintptr_t>(_dyld_get_image_header(i)) == base) {
            slide = _dyld_get_image_vmaddr_slide(i); found = true; break;
        }
    }
    if (!found) return {};
    Module module{base, Abi::LibcppArm64, {}};
    std::uintptr_t cursor = 0, end = 0;
    if (!safe_add(base, sizeof(header), cursor) || !safe_add(cursor, header.sizeofcmds, end)) return {};
    for (std::uint32_t i = 0; i < header.ncmds; ++i) {
        load_command command{};
        if (cursor > end || end - cursor < sizeof(command) || !reader.read(cursor, &command, sizeof(command))
            || command.cmdsize < sizeof(command) || command.cmdsize > end - cursor) return {};
        if (command.cmd == LC_SEGMENT_64) {
            segment_command_64 segment{};
            if (command.cmdsize < sizeof(segment) || !reader.read(cursor, &segment, sizeof(segment))) return {};
            std::uintptr_t start = 0, last = 0;
            if (segment.vmsize && (segment.initprot & VM_PROT_READ)) {
                if (!slid_address(slide, segment.vmaddr, start) || !safe_add(start, segment.vmsize - 1, last)) return {};
                module.regions.push_back({start, static_cast<std::size_t>(segment.vmsize), (segment.initprot & VM_PROT_EXECUTE) != 0});
            }
        }
        cursor += command.cmdsize;
    }
    return module.contains(base, sizeof(header)) ? module : Module{};
}
struct HostModule {
    Module module;
    Layout layout;
};
inline std::optional<Layout> inspect_layout(const Module& module, const char* path) {
    // Only inspect already-loaded host code. No private host function is called.
    void* handle = dlopen(path, RTLD_LAZY | RTLD_NOLOAD | RTLD_FIRST);
    if (!handle) return std::nullopt;
    const std::unique_ptr<void, decltype(&dlclose)> library(handle, dlclose);
    const Reader reader{mach_read};
    const auto read_code = [&](const char* symbol, arm64::Code& code) {
        auto* function = dlsym(handle, symbol);
        const auto address = reinterpret_cast<std::uintptr_t>(function);
        return function && address % alignof(arm64::Instruction) == 0
            && module.contains(address, sizeof(code), true) && reader.read(address, code.data(), sizeof(code));
    };
    arm64::Code source{}, image{}, message{};
    if (!read_code("_ZN17TEPluginProcessor20fetchFrameFromEditorERKNSt3__112basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEEx", source)
        || !read_code("_ZN25TEOfxhImageEffectInstance8populateENSt3__18functionIFN8veplugin9FrameDataERKNS0_12basic_stringIcNS0_11char_traitsIcEENS0_9allocatorIcEEEEdddddEEE", image)
        || !read_code("_ZN19TEOfxPluginInstance18setMessageCallbackERKNSt3__18functionIFbPKN8veplugin12MessageEventEEEE", message)) {
        return std::nullopt;
    }
    return arm64::discover(source, image, message);
}
inline std::shared_ptr<const HostModule> loaded_module(std::uintptr_t address) {
    Dl_info information{};
    if (!dladdr(reinterpret_cast<const void*>(address), &information) || !information.dli_fname || !information.dli_fbase) return {};
    const std::string path(information.dli_fname);
    const auto slash = path.find_last_of('/');
    if (path.substr(slash == std::string::npos ? 0 : slash + 1) != "libcccreator.dylib") return {};
    const auto base = reinterpret_cast<std::uintptr_t>(information.dli_fbase);
    static std::mutex mutex;
    static std::map<std::uintptr_t, std::shared_ptr<const HostModule>> cache;
    std::lock_guard<std::mutex> lock(mutex);
    const auto found = cache.find(base);
    if (found != cache.end()) return found->second;
    auto module = inspect_module(base);
    if (!module.base) return {};
    const auto layout = inspect_layout(module, information.dli_fname);
    // Cache the result for this loaded image, including unsupported layouts.
    std::shared_ptr<const HostModule> result;
    if (layout) result = std::make_shared<const HostModule>(HostModule{std::move(module), *layout});
    cache.emplace(base, result);
    return result;
}
} // namespace detail
inline bool is_pending(const Observation& observation) {
    return observation.status == Status::MissingMessageCallback || observation.status == Status::EmptySource;
}
inline bool module_supported() {
#if defined(__arm64__) || defined(__aarch64__)
    try {
        unsigned matches = 0;
        bool supported = false;
        for (std::uint32_t i = 0; i < _dyld_image_count(); ++i) {
            const char* name = _dyld_get_image_name(i);
            if (!name) continue;
            const std::string path(name);
            const auto slash = path.find_last_of('/');
            if (path.substr(slash == std::string::npos ? 0 : slash + 1) != "libcccreator.dylib") continue;
            ++matches;
            supported = bool(detail::loaded_module(reinterpret_cast<std::uintptr_t>(_dyld_get_image_header(i))));
        }
        return matches == 1 && supported;
    } catch (...) { return false; }
#else
    return false;
#endif
}
inline Observation inspect_source(const void* effect) {
#if defined(__arm64__) || defined(__aarch64__)
    try {
        if (!effect) return {Status::NullEffect, 0, {}};
        const jianying_source_contract::Reader reader{detail::mach_read};
        std::uintptr_t table = 0;
        const auto address = reinterpret_cast<std::uintptr_t>(effect);
        if (!reader.at(address, 0, table)) return {Status::UnreadableEffect, 0, {}};
        const auto host = detail::loaded_module(table);
        if (!host) return {Status::ModuleUnavailable, 0, {}};
        return jianying_source_contract::inspect(address, host->module, reader, host->layout);
    } catch (...) { return {Status::InternalFailure, 0, {}}; }
#else
    (void)effect;
    return {Status::ModuleUnavailable, 0, {}};
#endif
}
} // namespace jianying_macos_source_path
