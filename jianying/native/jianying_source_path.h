// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <bcrypt.h>
#include <array>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>
#pragma comment(lib, "bcrypt.lib")

namespace jianying_source_path {
constexpr char verified_sha256[] = "b6e9b8e2cc44fa7b48aa3b86a202c23734734882590cc66d16e8ab434d1dc08a";

inline bool read_local(std::uintptr_t address, void* output, std::size_t bytes) {
    SIZE_T copied = 0;
    return address && ReadProcessMemory(GetCurrentProcess(), reinterpret_cast<const void*>(address), output, bytes, &copied) && copied == bytes;
}

template<class T> inline bool read_local(std::uintptr_t address, T& output) {
    return read_local(address, &output, sizeof(output));
}

inline std::string file_sha256(const wchar_t* path) {
    HANDLE file = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                              nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return {};
    BCRYPT_ALG_HANDLE algorithm = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    std::array<unsigned char, 32> digest{};
    std::array<unsigned char, 65536> buffer{};
    std::vector<unsigned char> object;
    DWORD object_bytes = 0, returned = 0;
    bool ok = BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) >= 0;
    if (ok) ok = BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&object_bytes), sizeof(object_bytes), &returned, 0) >= 0;
    if (ok) {
        object.resize(object_bytes);
        ok = BCryptCreateHash(algorithm, &hash, object.data(), object_bytes, nullptr, 0, 0) >= 0;
    }
    while (ok) {
        DWORD bytes = 0;
        if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &bytes, nullptr)) { ok = false; break; }
        if (!bytes) break;
        ok = BCryptHashData(hash, buffer.data(), bytes, 0) >= 0;
    }
    if (ok) ok = BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) >= 0;
    if (hash) BCryptDestroyHash(hash);
    if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0);
    CloseHandle(file);
    if (!ok) return {};
    constexpr char hex[] = "0123456789abcdef";
    std::string result;
    result.reserve(64);
    for (unsigned char byte : digest) { result.push_back(hex[byte >> 4]); result.push_back(hex[byte & 15]); }
    return result;
}

inline std::uintptr_t verified_loaded_module() {
    static const std::uintptr_t verified = [] {
        HMODULE module = GetModuleHandleW(L"cccreator.dll");
        if (!module) return std::uintptr_t(0);
        std::array<wchar_t, 32768> path{};
        const DWORD length = GetModuleFileNameW(module, path.data(), static_cast<DWORD>(path.size()));
        if (!length || length >= path.size() || file_sha256(path.data()) != verified_sha256) return std::uintptr_t(0);
        return reinterpret_cast<std::uintptr_t>(module);
    }();
    return verified;
}

// This is the single private-ABI boundary; callers keep only the copied string.
inline bool read_verified_layout(std::uintptr_t effect, std::uintptr_t module, std::string& result) {
    result.clear();
    std::uintptr_t vtable = 0, message = 0, image = 0, processor = 0, image_processor = 0;
    if (!effect || !module || !read_local(effect, vtable) || vtable != module + 0x491c298) return false;
    if (!read_local(effect + 0x148, message) || !message || !read_local(message, vtable) || vtable != module + 0x48ee588) return false;
    if (!read_local(message + 8, processor) || !processor) return false;
    if (!read_local(effect + 0x1d0, image) || !image || !read_local(image, vtable) || vtable != module + 0x48ee610) return false;
    if (!read_local(image + 8, image_processor) || image_processor != processor) return false;
    struct StringLayout { std::array<char, 16> storage; std::uint64_t length, capacity; } source{};
    static_assert(sizeof(StringLayout) == 32);
    if (!read_local(processor + 0x108, source) || !source.length || source.length > 32767 || source.length > source.capacity) return false;
    result.resize(static_cast<std::size_t>(source.length));
    if (source.capacity < 16) {
        if (source.capacity != 15 || source.length >= 16) { result.clear(); return false; }
        result.assign(source.storage.data(), static_cast<std::size_t>(source.length));
    } else {
        std::uintptr_t content = 0;
        static_assert(sizeof(content) == sizeof(source.storage) / 2);
        std::memcpy(&content, source.storage.data(), sizeof(content));
        if (!read_local(content, result.data(), result.size())) { result.clear(); return false; }
    }
    if (result.find('\0') != std::string::npos || !MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, result.data(), static_cast<int>(result.size()), nullptr, 0)) {
        result.clear(); return false;
    }
    return true;
}

inline bool copy_source_path(const void* effect, std::string& result) {
    static_assert(sizeof(void*) == 8, "The verified host ABI is Windows x64 only");
    return read_verified_layout(reinterpret_cast<std::uintptr_t>(effect), verified_loaded_module(), result);
}

inline std::string copy_source_path(const void* effect) {
    std::string result;
    copy_source_path(effect, result);
    return result;
}
}
