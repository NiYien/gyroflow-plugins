// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <CommonCrypto/CommonDigest.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#include <dlfcn.h>
#include <array>
#include <cstdint>
#include <fstream>
#include <limits>
#include <map>
#include <mutex>
#include <string>
#include <vector>

namespace jianying_macos_source_path {
constexpr char verified_sha256[] = "8db71da2f613404b3c6ad5edd91d33af4ad877b54aa52dec3730d6cf3922d503";
constexpr std::uint64_t effect_vtable = 0x376c090;
constexpr std::uint64_t message_vtable = 0x3751420;
constexpr std::uint64_t image_vtable = 0x3751340;
constexpr std::size_t maximum_source_bytes = 32767;

enum class Status {
    Ready, UnsupportedArchitecture, NullEffect, AddressOverflow, UnreadableEffect, ModuleUnavailable,
    ModuleNameMismatch, ModuleHeaderMismatch, ModuleHashMismatch, EffectVtableMismatch,
    MissingMessageCallback, MissingImageCallback, UnreadableCallback, MessageVtableMismatch,
    ImageVtableMismatch, MissingProcessor, ProcessorMismatch, UnreadableSourceHeader,
    EmptySource, InvalidStringLayout, SourceTooLong, UnreadableSourcePayload,
    MissingTerminator, EmbeddedNull, InvalidUtf8, ChangedDuringCopy, InternalFailure
};
inline const char* status_name(Status status) {
    switch (status) {
#define GF_SOURCE_STATUS(value, label) case Status::value: return label
        GF_SOURCE_STATUS(Ready, "ready");
        GF_SOURCE_STATUS(UnsupportedArchitecture, "unsupported_architecture");
        GF_SOURCE_STATUS(NullEffect, "null_effect");
        GF_SOURCE_STATUS(AddressOverflow, "address_overflow");
        GF_SOURCE_STATUS(UnreadableEffect, "unreadable_effect");
        GF_SOURCE_STATUS(ModuleUnavailable, "module_unavailable");
        GF_SOURCE_STATUS(ModuleNameMismatch, "module_name_mismatch");
        GF_SOURCE_STATUS(ModuleHeaderMismatch, "module_header_mismatch");
        GF_SOURCE_STATUS(ModuleHashMismatch, "module_hash_mismatch");
        GF_SOURCE_STATUS(EffectVtableMismatch, "effect_vtable_mismatch");
        GF_SOURCE_STATUS(MissingMessageCallback, "missing_message_callback");
        GF_SOURCE_STATUS(MissingImageCallback, "missing_image_callback");
        GF_SOURCE_STATUS(UnreadableCallback, "unreadable_callback");
        GF_SOURCE_STATUS(MessageVtableMismatch, "message_vtable_mismatch");
        GF_SOURCE_STATUS(ImageVtableMismatch, "image_vtable_mismatch");
        GF_SOURCE_STATUS(MissingProcessor, "missing_processor");
        GF_SOURCE_STATUS(ProcessorMismatch, "processor_mismatch");
        GF_SOURCE_STATUS(UnreadableSourceHeader, "unreadable_source_header");
        GF_SOURCE_STATUS(EmptySource, "empty_source");
        GF_SOURCE_STATUS(InvalidStringLayout, "invalid_string_layout");
        GF_SOURCE_STATUS(SourceTooLong, "source_too_long");
        GF_SOURCE_STATUS(UnreadableSourcePayload, "unreadable_source_payload");
        GF_SOURCE_STATUS(MissingTerminator, "missing_terminator");
        GF_SOURCE_STATUS(EmbeddedNull, "embedded_null");
        GF_SOURCE_STATUS(InvalidUtf8, "invalid_utf8");
        GF_SOURCE_STATUS(ChangedDuringCopy, "changed_during_copy");
        GF_SOURCE_STATUS(InternalFailure, "internal_failure");
#undef GF_SOURCE_STATUS
    }
    return "internal_failure";
}
struct Observation {
    Status status = Status::InternalFailure;
    std::uintptr_t processor = 0;
    std::string source;
    bool operator==(const Observation& other) const {
        return status == other.status && processor == other.processor && source == other.source;
    }
};

namespace detail {
struct Reader {
    using Function = bool (*)(std::uintptr_t, void*, std::size_t, void*);
    Function function;
    void* context = nullptr;
    bool read(std::uintptr_t address, void* output, std::size_t bytes) const {
        if (!address || !output || !bytes || bytes - 1 > std::numeric_limits<std::uintptr_t>::max() - address)
            return false;
        return function && function(address, output, bytes, context);
    }
};
inline bool safe_add(std::uintptr_t base, std::uint64_t offset, std::uintptr_t& result) {
    if (offset > std::numeric_limits<std::uintptr_t>::max() - base) return false;
    result = base + static_cast<std::uintptr_t>(offset);
    return true;
}
inline bool slid_address(std::intptr_t slide, std::uint64_t address, std::uintptr_t& result) {
    if (slide >= 0) return safe_add(static_cast<std::uintptr_t>(slide), address, result);
    const auto magnitude = static_cast<std::uint64_t>(-(slide + 1)) + 1;
    if (address < magnitude || address - magnitude > std::numeric_limits<std::uintptr_t>::max()) return false;
    result = static_cast<std::uintptr_t>(address - magnitude);
    return true;
}
inline bool mach_read(std::uintptr_t address, void* output, std::size_t bytes, void*) {
    mach_vm_size_t copied = 0;
    return mach_vm_read_overwrite(mach_task_self(), static_cast<mach_vm_address_t>(address),
        static_cast<mach_vm_size_t>(bytes), reinterpret_cast<mach_vm_address_t>(output), &copied) == KERN_SUCCESS
        && copied == bytes;
}
inline bool read_word(const Reader& reader, std::uintptr_t base, std::uint64_t offset, std::uintptr_t& value) {
    std::uintptr_t address = 0;
    return safe_add(base, offset, address) && reader.read(address, &value, sizeof(value));
}
struct ModuleIdentity {
    Status status = Status::ModuleUnavailable;
    std::intptr_t slide = 0;
    std::uint32_t cpu_type = 0;
    std::uint32_t cpu_subtype = 0;
    std::string sha256;
};
inline std::string file_sha256(const char* path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) return {};
    CC_SHA256_CTX context;
    if (CC_SHA256_Init(&context) != 1) return {};
    std::array<char, 65536> bytes{};
    while (file) {
        file.read(bytes.data(), static_cast<std::streamsize>(bytes.size()));
        const auto count = file.gcount();
        if (count && CC_SHA256_Update(&context, bytes.data(), static_cast<CC_LONG>(count)) != 1) return {};
    }
    if (!file.eof()) return {};
    std::array<unsigned char, CC_SHA256_DIGEST_LENGTH> digest{};
    if (CC_SHA256_Final(digest.data(), &context) != 1) return {};
    constexpr char hex[] = "0123456789abcdef";
    std::string result;
    result.reserve(digest.size() * 2);
    for (const auto byte : digest) { result += hex[byte >> 4]; result += hex[byte & 15]; }
    return result;
}
inline ModuleIdentity loaded_module(std::uintptr_t vtable) {
    Dl_info information{};
    if (!dladdr(reinterpret_cast<const void*>(vtable), &information) || !information.dli_fname || !information.dli_fbase)
        return {};
    const std::string path(information.dli_fname);
    const auto separator = path.find_last_of('/');
    if (path.substr(separator == std::string::npos ? 0 : separator + 1) != "libcccreator.dylib")
        return {Status::ModuleNameMismatch, 0, 0, 0, {}};
    const auto base = reinterpret_cast<std::uintptr_t>(information.dli_fbase);
    static std::mutex cache_mutex;
    static std::map<std::pair<std::uintptr_t, std::string>, ModuleIdentity> cache;
    std::lock_guard<std::mutex> lock(cache_mutex);
    const auto key = std::make_pair(base, path);
    const auto cached = cache.find(key);
    if (cached != cache.end()) return cached->second;
    ModuleIdentity identity;
    mach_header_64 header{};
    const Reader reader{mach_read};
    if (!reader.read(base, &header, sizeof(header)) || header.magic != MH_MAGIC_64
        || header.cputype != CPU_TYPE_ARM64 || header.cpusubtype != CPU_SUBTYPE_ARM64_ALL) {
        identity.status = Status::ModuleHeaderMismatch;
    } else {
        identity.cpu_type = header.cputype;
        identity.cpu_subtype = header.cpusubtype;
        bool matched = false;
        for (std::uint32_t index = 0; index < _dyld_image_count(); ++index) {
            if (reinterpret_cast<std::uintptr_t>(_dyld_get_image_header(index)) == base) {
                identity.slide = _dyld_get_image_vmaddr_slide(index);
                matched = true;
                break;
            }
        }
        if (matched) {
            identity.sha256 = file_sha256(path.c_str());
            identity.status = identity.sha256 == verified_sha256 ? Status::Ready : Status::ModuleHashMismatch;
        }
    }
    cache.emplace(key, identity);
    return identity;
}
struct Chain {
    std::uintptr_t effect_table = 0, message = 0, image = 0;
    std::uintptr_t message_table = 0, image_table = 0;
    std::uintptr_t message_processor = 0, image_processor = 0;
    bool operator==(const Chain& other) const {
        return effect_table == other.effect_table && message == other.message && image == other.image
            && message_table == other.message_table && image_table == other.image_table
            && message_processor == other.message_processor && image_processor == other.image_processor;
    }
};
inline Status read_chain(std::uintptr_t effect, const ModuleIdentity& module, const Reader& reader, Chain& chain) {
    if (!effect) return Status::NullEffect;
    std::uintptr_t expected_effect = 0, expected_message = 0, expected_image = 0, last_field = 0;
    if (!slid_address(module.slide, effect_vtable, expected_effect)
        || !slid_address(module.slide, message_vtable, expected_message)
        || !slid_address(module.slide, image_vtable, expected_image)
        || !safe_add(effect, 0x1a8, last_field)) return Status::AddressOverflow;
    if (!read_word(reader, effect, 0, chain.effect_table)) return Status::UnreadableEffect;
    if (chain.effect_table != expected_effect) return Status::EffectVtableMismatch;
    if (!read_word(reader, effect, 0x128, chain.message)) return Status::UnreadableEffect;
    if (!read_word(reader, effect, 0x1a8, chain.image)) return Status::UnreadableEffect;
    if (!chain.image) return Status::MissingImageCallback;
    if (!read_word(reader, chain.image, 0, chain.image_table)
        || !read_word(reader, chain.image, 8, chain.image_processor)) return Status::UnreadableCallback;
    if (chain.image_table != expected_image) return Status::ImageVtableMismatch;
    if (!chain.image_processor) return Status::MissingProcessor;
    // populate installs the image callback before Create; only the later message callback may be pending.
    if (!chain.message) return Status::MissingMessageCallback;
    if (!read_word(reader, chain.message, 0, chain.message_table)
        || !read_word(reader, chain.message, 8, chain.message_processor)) return Status::UnreadableCallback;
    if (chain.message_table != expected_message) return Status::MessageVtableMismatch;
    if (!chain.message_processor) return Status::MissingProcessor;
    if (chain.message_processor != chain.image_processor) return Status::ProcessorMismatch;
    return Status::Ready;
}
inline std::uint64_t local_word(const std::array<unsigned char, 24>& bytes, std::size_t offset) {
    std::uint64_t result = 0;
    for (std::size_t index = 0; index < 8; ++index) result |= std::uint64_t(bytes[offset + index]) << (index * 8);
    return result;
}
inline bool valid_utf8(const std::vector<unsigned char>& bytes, std::size_t length) {
    for (std::size_t index = 0; index < length;) {
        const auto first = bytes[index++];
        if (first <= 0x7f) continue;
        unsigned remaining = 0;
        std::uint32_t codepoint = 0, minimum = 0;
        if (first >= 0xc2 && first <= 0xdf) { remaining = 1; codepoint = first & 0x1f; minimum = 0x80; }
        else if (first >= 0xe0 && first <= 0xef) { remaining = 2; codepoint = first & 0x0f; minimum = 0x800; }
        else if (first >= 0xf0 && first <= 0xf4) { remaining = 3; codepoint = first & 0x07; minimum = 0x10000; }
        else return false;
        if (remaining > length - index) return false;
        for (unsigned part = 0; part < remaining; ++part) {
            const auto next = bytes[index++];
            if ((next & 0xc0) != 0x80) return false;
            codepoint = (codepoint << 6) | (next & 0x3f);
        }
        if (codepoint < minimum || codepoint > 0x10ffff || (codepoint >= 0xd800 && codepoint <= 0xdfff)) return false;
    }
    return true;
}
inline Observation read_verified_layout(std::uintptr_t effect, const ModuleIdentity& module, const Reader& reader) {
    Observation result;
    if (!effect) { result.status = Status::NullEffect; return result; }
    if (module.status != Status::Ready) { result.status = module.status; return result; }
    if (module.sha256 != verified_sha256) { result.status = Status::ModuleHashMismatch; return result; }
    if (module.cpu_type != CPU_TYPE_ARM64 || module.cpu_subtype != CPU_SUBTYPE_ARM64_ALL) {
        result.status = Status::UnsupportedArchitecture; return result;
    }
    Chain first;
    result.status = read_chain(effect, module, reader, first);
    if (result.status != Status::Ready) return result;
    result.processor = first.message_processor;
    std::uintptr_t header_address = 0;
    if (!safe_add(result.processor, 0xf0, header_address)) { result.status = Status::AddressOverflow; return result; }
    std::array<unsigned char, 24> header{};
    if (!reader.read(header_address, header.data(), header.size())) { result.status = Status::UnreadableSourceHeader; return result; }
    const bool long_string = (header[23] & 0x80) != 0;
    const auto length = long_string ? local_word(header, 8) : std::uint64_t(header[23]);
    const auto capacity = long_string ? local_word(header, 16) & 0x7fffffffffffffffULL : 23;
    if (!length) { result.status = Status::EmptySource; return result; }
    if (length > maximum_source_bytes) { result.status = Status::SourceTooLong; return result; }
    if ((!long_string && length > 22) || (long_string && capacity <= length)) {
        result.status = Status::InvalidStringLayout; return result;
    }
    std::uintptr_t payload_address = long_string ? static_cast<std::uintptr_t>(local_word(header, 0)) : header_address;
    std::uintptr_t terminator_address = 0;
    if (!safe_add(payload_address, length, terminator_address)) { result.status = Status::AddressOverflow; return result; }
    std::vector<unsigned char> payload(static_cast<std::size_t>(length) + 1);
    if (long_string) {
        if (!reader.read(payload_address, payload.data(), payload.size())) { result.status = Status::UnreadableSourcePayload; return result; }
    } else {
        for (std::size_t index = 0; index < payload.size(); ++index) payload[index] = header[index];
    }
    Chain second;
    std::array<unsigned char, 24> second_header{};
    if (read_chain(effect, module, reader, second) != Status::Ready || !(first == second)
        || !reader.read(header_address, second_header.data(), second_header.size()) || header != second_header) {
        result.status = Status::ChangedDuringCopy; return result;
    }
    if (long_string) {
        std::vector<unsigned char> second_payload(payload.size());
        Chain final_chain;
        std::array<unsigned char, 24> final_header{};
        if (!reader.read(payload_address, second_payload.data(), second_payload.size()) || payload != second_payload
            || read_chain(effect, module, reader, final_chain) != Status::Ready || !(first == final_chain)
            || !reader.read(header_address, final_header.data(), final_header.size()) || header != final_header) {
            result.status = Status::ChangedDuringCopy; return result;
        }
    }
    if (payload.back() != 0) { result.status = Status::MissingTerminator; return result; }
    for (std::size_t index = 0; index < length; ++index) {
        if (!payload[index]) { result.status = Status::EmbeddedNull; return result; }
    }
    if (!valid_utf8(payload, static_cast<std::size_t>(length))) { result.status = Status::InvalidUtf8; return result; }
    result.source.assign(reinterpret_cast<const char*>(payload.data()), static_cast<std::size_t>(length));
    result.status = Status::Ready;
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
        const auto count = _dyld_image_count();
        for (std::uint32_t index = 0; index < count; ++index) {
            const char* name = _dyld_get_image_name(index);
            if (!name) continue;
            const std::string path(name);
            const auto separator = path.find_last_of('/');
            if (path.substr(separator == std::string::npos ? 0 : separator + 1) != "libcccreator.dylib") continue;
            ++matches;
            const auto module = detail::loaded_module(reinterpret_cast<std::uintptr_t>(_dyld_get_image_header(index)));
            supported = module.status == Status::Ready && module.sha256 == verified_sha256
                && module.cpu_type == CPU_TYPE_ARM64 && module.cpu_subtype == CPU_SUBTYPE_ARM64_ALL;
        }
        return matches == 1 && supported;
    } catch (...) { return false; }
#else
    return false;
#endif
}

// This is the only private macOS host ABI boundary; callers receive an owned string copy.
inline Observation inspect_source(const void* effect) {
#if defined(__arm64__) || defined(__aarch64__)
    try {
        if (!effect) return {Status::NullEffect, 0, {}};
        const detail::Reader reader{detail::mach_read};
        std::uintptr_t vtable = 0;
        if (!detail::read_word(reader, reinterpret_cast<std::uintptr_t>(effect), 0, vtable))
            return {Status::UnreadableEffect, 0, {}};
        const auto module = detail::loaded_module(vtable);
        return detail::read_verified_layout(reinterpret_cast<std::uintptr_t>(effect), module, reader);
    } catch (...) { return {Status::InternalFailure, 0, {}}; }
#else
    (void)effect;
    return {Status::UnsupportedArchitecture, 0, {}};
#endif
}

} // namespace jianying_macos_source_path
