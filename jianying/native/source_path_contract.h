// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <limits>
#include <string>
#include <vector>

namespace jianying_source_contract {
constexpr std::size_t maximum_source_bytes = 32767;
enum class Status {
    Ready, ModuleUnavailable, NullEffect, AddressOverflow, UnreadableEffect,
    EffectTypeMismatch, MissingMessageCallback, MissingImageCallback, UnreadableCallback,
    MessageTypeMismatch, ImageTypeMismatch, MissingProcessor, ProcessorMismatch,
    UnreadableSourceHeader, EmptySource, InvalidStringLayout, SourceTooLong,
    UnreadableSourcePayload, MissingTerminator, EmbeddedNull, InvalidUtf8,
    RelativeSource, ChangedDuringCopy, InternalFailure
};
inline const char* status_name(Status status) {
    switch (status) {
#define GF_SOURCE_STATUS(value, text) case Status::value: return text
        GF_SOURCE_STATUS(Ready, "ready");
        GF_SOURCE_STATUS(ModuleUnavailable, "module_unavailable");
        GF_SOURCE_STATUS(NullEffect, "null_effect");
        GF_SOURCE_STATUS(AddressOverflow, "address_overflow");
        GF_SOURCE_STATUS(UnreadableEffect, "unreadable_effect");
        GF_SOURCE_STATUS(EffectTypeMismatch, "effect_type_mismatch");
        GF_SOURCE_STATUS(MissingMessageCallback, "missing_message_callback");
        GF_SOURCE_STATUS(MissingImageCallback, "missing_image_callback");
        GF_SOURCE_STATUS(UnreadableCallback, "unreadable_callback");
        GF_SOURCE_STATUS(MessageTypeMismatch, "message_type_mismatch");
        GF_SOURCE_STATUS(ImageTypeMismatch, "image_type_mismatch");
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
        GF_SOURCE_STATUS(RelativeSource, "relative_source");
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
};
inline bool safe_add(std::uintptr_t base, std::uint64_t offset, std::uintptr_t& result) {
    if (offset > std::numeric_limits<std::uintptr_t>::max() - base) return false;
    result = base + static_cast<std::uintptr_t>(offset);
    return true;
}
struct Reader {
    using Function = bool (*)(std::uintptr_t, void*, std::size_t, void*);
    Function function;
    void* context = nullptr;
    bool read(std::uintptr_t address, void* output, std::size_t size) const {
        return address && output && size && size - 1 <= std::numeric_limits<std::uintptr_t>::max() - address
            && function && function(address, output, size, context);
    }
    template<class T> bool at(std::uintptr_t base, std::uint64_t offset, T& value) const {
        std::uintptr_t address = 0;
        return safe_add(base, offset, address) && read(address, &value, sizeof(value));
    }
};
enum class Abi { LibcppArm64, MsvcX64 };
struct Region { std::uintptr_t start; std::size_t size; bool executable; };
struct Module {
    std::uintptr_t base = 0;
    Abi abi = Abi::LibcppArm64;
    std::vector<Region> regions;
    std::size_t available(std::uintptr_t address, bool executable = false) const {
        for (const auto& region : regions) {
            if ((!executable || region.executable) && address >= region.start
                && address - region.start < region.size) return region.size - (address - region.start);
        }
        return 0;
    }
    bool contains(std::uintptr_t address, std::size_t bytes, bool executable = false) const {
        return bytes && available(address, executable) >= bytes;
    }
    template<class T> bool read(const Reader& reader, std::uintptr_t address, T& value) const {
        return contains(address, sizeof(value)) && reader.read(address, &value, sizeof(value));
    }
};
enum class Role { Effect, Message, Image };
inline bool read_type_name(const Reader& reader, const Module& module, std::uintptr_t address, std::string& name) {
    name.clear();
    while (name.size() < 768) {
        std::array<char, 32> bytes{};
        const auto count = std::min({bytes.size(), module.available(address), 768 - name.size()});
        if (!count || !reader.read(address, bytes.data(), count)) return false;
        for (std::size_t index = 0; index < count; ++index) {
            if (!bytes[index]) return !name.empty();
            if (static_cast<unsigned char>(bytes[index]) < 32 || static_cast<unsigned char>(bytes[index]) > 126) return false;
            name.push_back(bytes[index]);
        }
        if (!safe_add(address, count, address)) return false;
    }
    return false;
}
inline bool matches_type_name(const std::string& name, Abi abi, Role role) {
    if (role == Role::Effect) return abi == Abi::LibcppArm64 ? name == "25TEOfxhImageEffectInstance"
        : name == ".?AVTEOfxhImageEffectInstance@@" || name == ".?AUTEOfxhImageEffectInstance@@";
    if (abi == Abi::LibcppArm64) {
        const std::string prefix = "NSt3__110__function6__funcIZN17TEPluginProcessor4initERKNS_10shared_ptrI17TESharedGLContextEEE";
        if (name.compare(0, prefix.size(), prefix) != 0) return false;
        // Clang has both numbered and signature-based lambda manglings; identify the owner and signature.
        return role == Role::Message ? name.find("12MessageEventE") != std::string::npos
                && (name.find("EFbPKN8veplugin") != std::string::npos || name.find("_Fb") != std::string::npos)
            : name.find("9FrameDataE") != std::string::npos && name.find("basic_stringIcNS_11char_traitsIcE") != std::string::npos
                && name.size() >= 8 && name.compare(name.size() - 8, 8, "dddddEEE") == 0;
    }
    // The no-allocator callable stores its single captured processor immediately after its vptr.
    return name.rfind(".?AV?$_Func_impl_no_alloc@", 0) == 0
        && name.find("init@TEPluginProcessor@@") != std::string::npos
        && name.find(role == Role::Message ? "MessageEvent@veplugin@@" : "FrameData@veplugin@@") != std::string::npos;
}
inline bool matches_type(const Reader& reader, const Module& module, std::uintptr_t vtable, Role role) {
    static_assert(sizeof(std::uintptr_t) == 8, "The source contract requires a 64-bit host");
    if (vtable < 16) return false;
    std::array<std::uintptr_t, 2> functions{};
    if (!module.read(reader, vtable, functions)
        || !module.contains(functions[0], 1, true) || !module.contains(functions[1], 1, true)) return false;
    std::uintptr_t name_address = 0;
    if (module.abi == Abi::LibcppArm64) {
        std::array<std::uintptr_t, 2> prefix{}, typeinfo{};
        if (!module.read(reader, vtable - 16, prefix) || prefix[0] != 0
            || !module.read(reader, prefix[1], typeinfo)) return false;
        // libc++ can tag a non-unique RTTI name with the top bit on native arm64.
        name_address = typeinfo[1] & 0x7fffffffffffffffULL;
    } else {
        std::uintptr_t locator = 0;
        std::array<std::uint32_t, 6> fields{};
        if (!module.read(reader, vtable - 8, locator) || !module.read(reader, locator, fields)
            || fields[0] != 1 || fields[1] != 0 || fields[2] != 0) return false;
        std::uintptr_t self = 0, typeinfo = 0, hierarchy = 0;
        if (!safe_add(module.base, fields[5], self) || self != locator
            || !safe_add(module.base, fields[3], typeinfo) || !module.contains(typeinfo, 17)
            || !safe_add(module.base, fields[4], hierarchy) || !module.contains(hierarchy, 4)
            || !safe_add(typeinfo, 16, name_address)) return false;
    }
    std::string name;
    return read_type_name(reader, module, name_address, name) && matches_type_name(name, module.abi, role);
}
struct Layout { std::size_t message, image, source; };
inline Layout layout(Abi abi) {
    // Keep the known relative member layout while validating object roles and stable snapshots.
    return abi == Abi::LibcppArm64 ? Layout{0x128, 0x1a8, 0xf0} : Layout{0x148, 0x1d0, 0x108};
}
struct Chain {
    std::uintptr_t effect_table = 0, message = 0, image = 0;
    std::uintptr_t message_table = 0, image_table = 0, message_processor = 0, image_processor = 0;
    bool operator==(const Chain& b) const {
        return effect_table == b.effect_table && message == b.message && image == b.image
            && message_table == b.message_table && image_table == b.image_table
            && message_processor == b.message_processor && image_processor == b.image_processor;
    }
};
inline Status read_chain(const Reader& reader, const Module& module, std::uintptr_t effect, Chain& chain) {
    std::uintptr_t end = 0;
    const auto offsets = layout(module.abi);
    if (!safe_add(effect, offsets.image + 7, end)) return Status::AddressOverflow;
    if (!reader.at(effect, 0, chain.effect_table)) return Status::UnreadableEffect;
    if (!matches_type(reader, module, chain.effect_table, Role::Effect)) return Status::EffectTypeMismatch;
    if (!reader.at(effect, offsets.message, chain.message) || !reader.at(effect, offsets.image, chain.image)) return Status::UnreadableEffect;
    if (!chain.image) return Status::MissingImageCallback;
    if (!reader.at(chain.image, 0, chain.image_table) || !reader.at(chain.image, 8, chain.image_processor)) return Status::UnreadableCallback;
    if (!matches_type(reader, module, chain.image_table, Role::Image)) return Status::ImageTypeMismatch;
    if (!chain.image_processor) return Status::MissingProcessor;
    if (!chain.message) return Status::MissingMessageCallback;
    if (!reader.at(chain.message, 0, chain.message_table) || !reader.at(chain.message, 8, chain.message_processor)) return Status::UnreadableCallback;
    if (!matches_type(reader, module, chain.message_table, Role::Message)) return Status::MessageTypeMismatch;
    if (!chain.message_processor) return Status::MissingProcessor;
    return chain.message_processor == chain.image_processor ? Status::Ready : Status::ProcessorMismatch;
}
inline std::uint64_t word(const std::array<unsigned char, 32>& bytes, std::size_t offset) {
    std::uint64_t result = 0;
    for (unsigned i = 0; i < 8; ++i) result |= std::uint64_t(bytes[offset + i]) << (i * 8);
    return result;
}
inline bool valid_utf8(const std::string& bytes) {
    for (std::size_t i = 0; i < bytes.size();) {
        const auto first = static_cast<unsigned char>(bytes[i++]);
        if (first <= 127) continue;
        unsigned count = 0;
        std::uint32_t codepoint = 0, minimum = 0;
        if (first >= 0xc2 && first <= 0xdf) { count = 1; codepoint = first & 31; minimum = 128; }
        else if (first >= 0xe0 && first <= 0xef) { count = 2; codepoint = first & 15; minimum = 2048; }
        else if (first >= 0xf0 && first <= 0xf4) { count = 3; codepoint = first & 7; minimum = 65536; }
        else return false;
        if (count > bytes.size() - i) return false;
        while (count--) {
            const auto next = static_cast<unsigned char>(bytes[i++]);
            if ((next & 0xc0) != 0x80) return false;
            codepoint = (codepoint << 6) | (next & 63);
        }
        if (codepoint < minimum || codepoint > 0x10ffff || (codepoint >= 0xd800 && codepoint <= 0xdfff)) return false;
    }
    return true;
}
inline bool absolute_source(const std::string& source, Abi abi) {
    if (source.compare(0, 8, "file:///") == 0) return true;
    if (abi == Abi::LibcppArm64) return !source.empty() && source[0] == '/';
    return (source.size() >= 3 && ((source[0] >= 'A' && source[0] <= 'Z') || (source[0] >= 'a' && source[0] <= 'z'))
        && source[1] == ':' && (source[2] == '/' || source[2] == '\\'))
        || (source.size() > 4 && source[0] == '\\' && source[1] == '\\' && source[2] != '\\');
}
inline Observation inspect(std::uintptr_t effect, const Module& module, const Reader& reader) {
    Observation result;
    if (!effect) { result.status = Status::NullEffect; return result; }
    if (!module.base || module.regions.empty()) { result.status = Status::ModuleUnavailable; return result; }
    Chain first;
    result.status = read_chain(reader, module, effect, first);
    if (result.status != Status::Ready) return result;
    result.processor = first.message_processor;
    std::uintptr_t address = 0;
    if (!safe_add(result.processor, layout(module.abi).source, address)) { result.status = Status::AddressOverflow; return result; }
    const bool mac = module.abi == Abi::LibcppArm64;
    const std::size_t header_size = mac ? 24 : 32;
    std::array<unsigned char, 32> header{};
    if (!reader.read(address, header.data(), header_size)) { result.status = Status::UnreadableSourceHeader; return result; }
    const bool long_string = mac ? (header[23] & 128) != 0 : word(header, 24) >= 16;
    const auto length = mac ? (long_string ? word(header, 8) : std::uint64_t(header[23])) : word(header, 16);
    const auto capacity = mac ? (long_string ? word(header, 16) & 0x7fffffffffffffffULL : 23) : word(header, 24);
    if (!length) { result.status = Status::EmptySource; return result; }
    if (length > maximum_source_bytes) { result.status = Status::SourceTooLong; return result; }
    if (mac ? ((!long_string && length > 22) || (long_string && capacity <= length))
        : ((!long_string && (capacity != 15 || length >= 16)) || length > capacity)) {
        result.status = Status::InvalidStringLayout; return result;
    }
    const auto payload_address = long_string ? static_cast<std::uintptr_t>(word(header, 0)) : address;
    std::uintptr_t terminator = 0;
    if (!safe_add(payload_address, length, terminator)) { result.status = Status::AddressOverflow; return result; }
    std::vector<char> payload(static_cast<std::size_t>(length) + 1);
    if (long_string) {
        if (!reader.read(payload_address, payload.data(), payload.size())) { result.status = Status::UnreadableSourcePayload; return result; }
    } else std::memcpy(payload.data(), header.data(), payload.size());
    Chain second;
    std::array<unsigned char, 32> header_after{};
    if (read_chain(reader, module, effect, second) != Status::Ready || !(first == second)
        || !reader.read(address, header_after.data(), header_size) || header != header_after) {
        result.status = Status::ChangedDuringCopy; return result;
    }
    if (long_string) {
        std::vector<char> payload_after(payload.size());
        Chain final_chain;
        if (!reader.read(payload_address, payload_after.data(), payload_after.size()) || payload != payload_after
            || read_chain(reader, module, effect, final_chain) != Status::Ready || !(first == final_chain)
            || !reader.read(address, header_after.data(), header_size) || header != header_after) {
            result.status = Status::ChangedDuringCopy; return result;
        }
    }
    if (payload.back()) { result.status = Status::MissingTerminator; return result; }
    const std::string source(payload.data(), static_cast<std::size_t>(length));
    if (source.find('\0') != std::string::npos) { result.status = Status::EmbeddedNull; return result; }
    if (!valid_utf8(source)) { result.status = Status::InvalidUtf8; return result; }
    if (!absolute_source(source, module.abi)) { result.status = Status::RelativeSource; return result; }
    result.source = source;
    result.status = Status::Ready;
    return result;
}
} // namespace jianying_source_contract
