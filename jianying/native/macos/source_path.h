// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "../source_path_contract.h"
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#include <dlfcn.h>
#include <map>
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
inline Module loaded_module(std::uintptr_t address) {
    Dl_info information{};
    if (!dladdr(reinterpret_cast<const void*>(address), &information) || !information.dli_fname || !information.dli_fbase) return {};
    const std::string path(information.dli_fname);
    const auto slash = path.find_last_of('/');
    if (path.substr(slash == std::string::npos ? 0 : slash + 1) != "libcccreator.dylib") return {};
    const auto base = reinterpret_cast<std::uintptr_t>(information.dli_fbase);
    static std::mutex mutex;
    static std::map<std::uintptr_t, Module> cache;
    std::lock_guard<std::mutex> lock(mutex);
    const auto found = cache.find(base);
    if (found != cache.end()) return found->second;
    const auto module = inspect_module(base);
    // A failed or incomplete load must remain retryable.
    if (module.base) cache.emplace(base, module);
    return module;
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
            supported = detail::loaded_module(reinterpret_cast<std::uintptr_t>(_dyld_get_image_header(i))).base != 0;
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
        return jianying_source_contract::inspect(address, detail::loaded_module(table), reader);
    } catch (...) { return {Status::InternalFailure, 0, {}}; }
#else
    (void)effect;
    return {Status::ModuleUnavailable, 0, {}};
#endif
}
} // namespace jianying_macos_source_path
