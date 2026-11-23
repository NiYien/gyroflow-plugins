// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include "windows_source_layout.h"
#include <cwctype>
#include <map>
#include <memory>
#include <mutex>

namespace jianying_source_path {
using jianying_source_contract::Observation;
using jianying_source_contract::Status;
using jianying_source_contract::status_name;

namespace detail {
using namespace jianying_source_contract;

inline bool read_local(std::uintptr_t address, void* output, std::size_t size, void*) {
    SIZE_T copied = 0;
    return ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<const void*>(address), output, size, &copied) && copied == size;
}

inline bool is_cccreator_module(HMODULE handle) {
    wchar_t path[32768]{};
    const auto length = GetModuleFileNameW(handle, path, static_cast<DWORD>(std::size(path)));
    if (!length || length >= std::size(path)) return false;
    std::wstring name(path, length);
    const auto slash = name.find_last_of(L"\\/");
    if (slash != std::wstring::npos) name.erase(0, slash + 1);
    for (auto& ch : name) ch = static_cast<wchar_t>(std::towlower(ch));
    return name == L"cccreator.dll";
}

struct ModuleKey {
    std::uintptr_t base = 0;
    std::uint32_t image_size = 0;
    std::uint32_t timestamp = 0;
    bool operator<(const ModuleKey& other) const {
        if (base != other.base) return base < other.base;
        if (image_size != other.image_size) return image_size < other.image_size;
        return timestamp < other.timestamp;
    }
};

struct HostImage {
    Module module;
    std::vector<x64::FunctionRange> pdata;
    std::mutex layout_mutex;
    std::map<std::uintptr_t, std::vector<std::size_t>> effect_slots;
    std::map<std::uintptr_t, std::optional<std::size_t>> source_offsets;
};

inline bool parse_pe_headers(std::uintptr_t base, const Reader& reader, Module& module,
    std::vector<x64::FunctionRange>& pdata, ModuleKey& key) {
    IMAGE_DOS_HEADER dos{};
    if (!reader.read(base, &dos, sizeof(dos)) || dos.e_magic != IMAGE_DOS_SIGNATURE
        || dos.e_lfanew < static_cast<LONG>(sizeof(dos)) || dos.e_lfanew > 1024 * 1024) return false;
    std::uintptr_t address = 0;
    IMAGE_NT_HEADERS64 header{};
    if (!safe_add(base, static_cast<std::uint64_t>(dos.e_lfanew), address) || !reader.read(address, &header, sizeof(header))
        || header.Signature != IMAGE_NT_SIGNATURE || header.FileHeader.Machine != IMAGE_FILE_MACHINE_AMD64
        || header.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC || header.FileHeader.NumberOfSections > 96
        || header.FileHeader.SizeOfOptionalHeader != sizeof(IMAGE_OPTIONAL_HEADER64)) return false;
    const auto image_size = header.OptionalHeader.SizeOfImage;
    if (!image_size || !safe_add(address, sizeof(header), address)) return false;
    key = {base, image_size, header.FileHeader.TimeDateStamp};
    module = {base, Abi::MsvcX64, {}};
    for (unsigned i = 0; i < header.FileHeader.NumberOfSections; ++i) {
        IMAGE_SECTION_HEADER section{};
        if (!reader.at(address, i * sizeof(section), section)) return false;
        const auto size = std::max(section.Misc.VirtualSize, section.SizeOfRawData);
        if (section.VirtualAddress >= image_size || size > image_size - section.VirtualAddress) return false;
        std::uintptr_t start = 0;
        if (!safe_add(base, section.VirtualAddress, start)) return false;
        if (size && (section.Characteristics & IMAGE_SCN_MEM_READ)) {
            module.regions.push_back({start, size, (section.Characteristics & IMAGE_SCN_MEM_EXECUTE) != 0});
        }
    }
    if (module.regions.empty()) return false;

    pdata.clear();
    if (header.OptionalHeader.NumberOfRvaAndSizes > IMAGE_DIRECTORY_ENTRY_EXCEPTION) {
        const auto& directory = header.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXCEPTION];
        std::uintptr_t table_address = 0;
        if (directory.VirtualAddress && directory.Size >= 12 && directory.Size <= 64u * 1024u * 1024u
            && safe_add(base, directory.VirtualAddress, table_address)
            && module.contains(table_address, directory.Size)) {
            const auto count = directory.Size / 12;
            std::vector<std::uint32_t> raw(count * 3);
            if (reader.read(table_address, raw.data(), count * 12)) {
                pdata.reserve(count);
                for (std::size_t i = 0; i < count; ++i) {
                    const auto begin = raw[i * 3];
                    const auto end = raw[i * 3 + 1];
                    if (end > begin) pdata.push_back({begin, end});
                }
                const auto by_begin = [](const x64::FunctionRange& a, const x64::FunctionRange& b) {
                    return a.begin < b.begin;
                };
                if (!std::is_sorted(pdata.begin(), pdata.end(), by_begin)) {
                    std::sort(pdata.begin(), pdata.end(), by_begin);
                }
            }
        }
    }
    return true;
}

inline std::shared_ptr<HostImage> host_image_for(std::uintptr_t vtable_address) {
    HMODULE handle = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        reinterpret_cast<LPCWSTR>(vtable_address), &handle) || !handle || !is_cccreator_module(handle)) {
        return {};
    }
    const auto base = reinterpret_cast<std::uintptr_t>(handle);
    const Reader reader{read_local};
    IMAGE_DOS_HEADER dos{};
    IMAGE_NT_HEADERS64 header{};
    std::uintptr_t nt_address = 0;
    if (!reader.read(base, &dos, sizeof(dos)) || dos.e_magic != IMAGE_DOS_SIGNATURE
        || dos.e_lfanew < static_cast<LONG>(sizeof(dos)) || dos.e_lfanew > 1024 * 1024
        || !safe_add(base, static_cast<std::uint64_t>(dos.e_lfanew), nt_address)
        || !reader.read(nt_address, &header, sizeof(header)) || header.Signature != IMAGE_NT_SIGNATURE) {
        return {};
    }
    const ModuleKey key{base, header.OptionalHeader.SizeOfImage, header.FileHeader.TimeDateStamp};

    static std::mutex cache_mutex;
    static std::map<ModuleKey, std::shared_ptr<HostImage>> cache;
    std::lock_guard<std::mutex> lock(cache_mutex);
    const auto existing = cache.find(key);
    if (existing != cache.end()) return existing->second;

    auto image = std::make_shared<HostImage>();
    ModuleKey parsed_key;
    if (!parse_pe_headers(base, reader, image->module, image->pdata, parsed_key)) {
        cache.emplace(key, nullptr);
        return {};
    }
    cache.emplace(key, image);
    return image;
}

inline std::vector<std::size_t> cached_effect_slots(HostImage& host, const Reader& reader, std::uintptr_t effect_vtable) {
    std::lock_guard<std::mutex> lock(host.layout_mutex);
    const auto found = host.effect_slots.find(effect_vtable);
    if (found != host.effect_slots.end()) return found->second;
    auto slots = x64::discover_effect_function_slots(reader, host.module, host.pdata, effect_vtable);
    host.effect_slots.emplace(effect_vtable, slots);
    return slots;
}

inline std::optional<std::size_t> cached_source_offset(HostImage& host, const Reader& reader, std::uintptr_t image_vtable) {
    std::lock_guard<std::mutex> lock(host.layout_mutex);
    const auto found = host.source_offsets.find(image_vtable);
    if (found != host.source_offsets.end()) return found->second;
    const auto offset = x64::discover_source_offset(reader, host.module, host.pdata, image_vtable);
    host.source_offsets.emplace(image_vtable, offset);
    return offset;
}
} // namespace detail

inline Observation inspect_source(const void* effect) {
    static_assert(sizeof(void*) == 8, "The source contract requires Windows x64");
    try {
        if (!effect) return {Status::NullEffect, 0, {}};
        const jianying_source_contract::Reader reader{detail::read_local};
        const auto address = reinterpret_cast<std::uintptr_t>(effect);
        std::uintptr_t effect_vtable = 0;
        if (!reader.at(address, 0, effect_vtable)) return {Status::UnreadableEffect, 0, {}};
        const auto host = detail::host_image_for(effect_vtable);
        if (!host) return {Status::ModuleUnavailable, 0, {}};
        if (!jianying_source_contract::matches_type(reader, host->module, effect_vtable, jianying_source_contract::Role::Effect)) {
            return {Status::EffectTypeMismatch, 0, {}};
        }

        const auto slots = detail::cached_effect_slots(*host, reader, effect_vtable);
        const auto callbacks = x64::identify_callback_slots(reader, host->module, address, slots);
        if (callbacks.status != Status::Ready) return {callbacks.status, 0, {}};

        const auto source_offset = detail::cached_source_offset(*host, reader, callbacks.image_vtable);
        if (!source_offset) return {Status::ModuleUnavailable, 0, {}};

        const jianying_source_contract::Layout layout{callbacks.message_offset, callbacks.image_offset, *source_offset};
        return jianying_source_contract::inspect(address, host->module, reader, layout);
    } catch (...) {
        return {Status::InternalFailure, 0, {}};
    }
}

inline bool copy_source_path(const void* effect, std::string& result) {
    result.clear();
    const auto observation = inspect_source(effect);
    if (observation.status != Status::Ready) return false;
    result = observation.source;
    return true;
}

inline std::string copy_source_path(const void* effect) {
    std::string result;
    copy_source_path(effect, result);
    return result;
}
} // namespace jianying_source_path
