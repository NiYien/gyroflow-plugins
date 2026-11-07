// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include "source_path_contract.h"

namespace jianying_source_path {
inline bool read_local(std::uintptr_t address, void* output, std::size_t size, void*) {
    SIZE_T copied = 0;
    return ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<const void*>(address), output, size, &copied) && copied == size;
}
inline jianying_source_contract::Module loaded_module() {
    using namespace jianying_source_contract;
    const auto base = reinterpret_cast<std::uintptr_t>(GetModuleHandleW(L"cccreator.dll"));
    const Reader reader{read_local};
    IMAGE_DOS_HEADER dos{};
    if (!reader.read(base, &dos, sizeof(dos)) || dos.e_magic != IMAGE_DOS_SIGNATURE
        || dos.e_lfanew < static_cast<LONG>(sizeof(dos)) || dos.e_lfanew > 1024 * 1024) return {};
    std::uintptr_t address = 0;
    IMAGE_NT_HEADERS64 header{};
    if (!safe_add(base, static_cast<std::uint64_t>(dos.e_lfanew), address) || !reader.read(address, &header, sizeof(header))
        || header.Signature != IMAGE_NT_SIGNATURE || header.FileHeader.Machine != IMAGE_FILE_MACHINE_AMD64
        || header.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC || header.FileHeader.NumberOfSections > 96
        || header.FileHeader.SizeOfOptionalHeader != sizeof(IMAGE_OPTIONAL_HEADER64)) return {};
    const auto image_size = header.OptionalHeader.SizeOfImage;
    if (!image_size || !safe_add(address, sizeof(header), address)) return {};
    Module module{base, Abi::MsvcX64, {}};
    for (unsigned i = 0; i < header.FileHeader.NumberOfSections; ++i) {
        IMAGE_SECTION_HEADER section{};
        if (!reader.at(address, i * sizeof(section), section)) return {};
        const auto size = std::max(section.Misc.VirtualSize, section.SizeOfRawData);
        if (section.VirtualAddress >= image_size || size > image_size - section.VirtualAddress) return {};
        std::uintptr_t start = 0;
        if (!safe_add(base, section.VirtualAddress, start)) return {};
        if (size && (section.Characteristics & IMAGE_SCN_MEM_READ)) {
            module.regions.push_back({start, size, (section.Characteristics & IMAGE_SCN_MEM_EXECUTE) != 0});
        }
    }
    return module;
}
inline bool copy_source_path(const void* effect, std::string& result) {
    static_assert(sizeof(void*) == 8, "The source contract requires Windows x64");
    result.clear();
    try {
        const auto observation = jianying_source_contract::inspect(reinterpret_cast<std::uintptr_t>(effect),
            loaded_module(), {read_local});
        if (observation.status != jianying_source_contract::Status::Ready) return false;
        result = observation.source;
        return true;
    } catch (...) { return false; }
}
inline std::string copy_source_path(const void* effect) {
    std::string result;
    copy_source_path(effect, result);
    return result;
}
} // namespace jianying_source_path
