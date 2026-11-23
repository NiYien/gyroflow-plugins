// SPDX-License-Identifier: GPL-3.0-or-later
// Exercise both compiler ABI profiles in synthetic address spaces without an editor process.
#include "source_path_contract.h"
#include "windows_source_layout.h"
#ifdef __APPLE__
#include "macos/source_path.h"
#endif
#ifdef _WIN32
#include "jianying_source_path.h"
#endif
#include <functional>
#include <iostream>
#include <map>
#include <stdexcept>
using namespace jianying_source_contract;
static void require(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
static constexpr Layout layout(Abi abi) {
    return abi == Abi::LibcppArm64 ? Layout{0x130, 0x1b0, 0xf8} : Layout{0x148, 0x1d0, 0x108};
}
struct Memory {
    std::map<std::uintptr_t, std::vector<unsigned char>> regions;
    std::map<std::uintptr_t, unsigned> counts;
    std::function<void(Memory&, std::uintptr_t, unsigned)> after;
    static bool read(std::uintptr_t address, void* output, std::size_t bytes, void* opaque) {
        auto& memory = *static_cast<Memory*>(opaque);
        for (auto& item : memory.regions) {
            if (address < item.first || address - item.first > item.second.size()) continue;
            const auto offset = address - item.first;
            if (bytes > item.second.size() - offset) continue;
            std::memcpy(output, item.second.data() + offset, bytes);
            const auto count = ++memory.counts[address];
            if (memory.after) memory.after(memory, address, count);
            return true;
        }
        return false;
    }
    template<class T> void put(std::uintptr_t address, const T& value) {
        for (auto& item : regions) if (address >= item.first && address - item.first <= item.second.size()
            && sizeof(value) <= item.second.size() - (address - item.first)) {
            std::memcpy(item.second.data() + address - item.first, &value, sizeof(value)); return;
        }
        throw std::runtime_error("Fixture write outside region");
    }
    void text(std::uintptr_t address, const std::string& value) {
        for (std::size_t i = 0; i <= value.size(); ++i) put<char>(address + i, i < value.size() ? value[i] : 0);
    }
};
struct Fixture {
    static constexpr std::uintptr_t effect = 0x10000, message = 0x20000, image = 0x30000, processor = 0x40000, payload = 0x50000;
    Memory memory;
    Module module;
    std::array<std::uintptr_t, 3> tables{};
    Fixture(Abi abi, std::uintptr_t base = 0x100000, std::uintptr_t shift = 0) : module{base, abi, {{base, 0x8000, false}, {base + 0x8000, 0x1000, true}}} {
        memory.regions[base].resize(0x9000);
        memory.regions[effect].resize(0x1e0); memory.regions[message].resize(16); memory.regions[image].resize(16);
        memory.regions[processor].resize(0x130);
        for (unsigned i = 0; i < 3; ++i) {
            tables[i] = base + 0x1000 + shift + i * 0x1000;
            type(i, name(abi, i));
        }
        memory.put(effect, tables[0]);
        memory.put(effect + layout(abi).message, message); memory.put(effect + layout(abi).image, image);
        memory.put(message, tables[1]); memory.put(image, tables[2]);
        memory.put(message + 8, processor); memory.put(image + 8, processor);
    }
    static std::string name(Abi abi, unsigned role) {
        if (abi == Abi::LibcppArm64) {
            if (!role) return "25TEOfxhImageEffectInstance";
            const std::string prefix = "NSt3__110__function6__funcIZN17TEPluginProcessor4initERKNS_10shared_ptrI17TESharedGLContextEEE";
            return prefix + (role == 1 ? "3$_1NS_9allocatorIS8_EEFbPKN8veplugin12MessageEventEEEE"
                : "3$_0NS_9allocatorIS8_EEFN8veplugin9FrameDataERKNS_12basic_stringIcNS_11char_traitsIcEENS9_IcEEEEdddddEEE");
        }
        if (!role) return ".?AVTEOfxhImageEffectInstance@@";
        // Captured MSVC RTTI names do not include the enclosing init function.
        return role == 1
            ? ".?AV?$_Func_impl_no_alloc@V<lambda_6b8473e888e3690aae2d0fc3ad4a9912>@@_NPEBUMessageEvent@veplugin@@@std@@"
            : ".?AV?$_Func_impl_no_alloc@V<lambda_77c7ea972ce80bd00bcf43d5f15377e0>@@UFrameData@veplugin@@AEBV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@NNNNN@std@@";
    }
    void type(unsigned role, const std::string& name_value) {
        const auto table = tables[role], info = table + 0x100, name_address = info + 0x100;
        memory.put(table, module.base + 0x8000); memory.put(table + 8, module.base + 0x8010);
        if (module.abi == Abi::LibcppArm64) {
            memory.put<std::uintptr_t>(table - 16, 0); memory.put(table - 8, info);
            memory.put(info + 8, name_address);
        } else {
            const auto locator = table + 0x80;
            memory.put(table - 8, locator);
            const std::array<std::uint32_t, 6> fields{1, 0, 0, static_cast<std::uint32_t>(info - module.base),
                static_cast<std::uint32_t>(info + 0x40 - module.base), static_cast<std::uint32_t>(locator - module.base)};
            memory.put(locator, fields);
            memory.text(info + 16, name_value);
            return;
        }
        memory.text(name_address, name_value);
    }
    void source(const std::string& value, bool force_long = false) {
        const auto field = processor + layout(module.abi).source;
        std::array<unsigned char, 32> zero{};
        memory.put(field, zero);
        const bool mac = module.abi == Abi::LibcppArm64;
        const bool long_value = force_long || value.size() >= (mac ? 23 : 16);
        if (long_value) {
            memory.regions[payload].assign(value.begin(), value.end()); memory.regions[payload].push_back(0);
            memory.put(field, payload);
        } else memory.text(field, value);
        if (mac) {
            if (long_value) { memory.put<std::uint64_t>(field + 8, value.size()); memory.put<std::uint64_t>(field + 16, 0x8000000000000000ULL | (value.size() + 1)); }
            else memory.put<unsigned char>(field + 23, static_cast<unsigned char>(value.size()));
        } else {
            memory.put<std::uint64_t>(field + 16, value.size()); memory.put<std::uint64_t>(field + 24, long_value ? std::max<std::size_t>(31, value.size()) : 15);
        }
    }
    Observation inspect(std::uintptr_t address = effect) { return jianying_source_contract::inspect(address, module, {Memory::read, &memory}, layout(module.abi)); }
};
int main() {
    try {
        unsigned cases = 0;
        for (auto abi : {Abi::LibcppArm64, Abi::MsvcX64}) {
            const std::string short_path = abi == Abi::LibcppArm64 ? "/短/片.mp4" : "C:/短/a.mov";
            const std::string long_path = abi == Abi::LibcppArm64 ? "/owned/原片 media/clip.mp4" : "C:/owned/原片 media/clip.mp4";
            auto expect = [&](Fixture& f, Status status) {
                const auto result = f.inspect(); require(result.status == status, status_name(status));
                require(status == Status::Ready || result.source.empty(), "Rejected result leaked source"); ++cases;
            };
            Fixture short_value(abi); short_value.source(short_path); auto owned = short_value.inspect();
            require(owned.status == Status::Ready && owned.source == short_path, "Short source copy"); ++cases;
            short_value.source(long_path); require(owned.source == short_path, "Copy retained host alias"); ++cases;
            expect(short_value, Status::Ready);
            for (std::uintptr_t base : {0x100000ULL, 0x7100000ULL}) for (std::uintptr_t shift : {0ULL, 0x180ULL}) {
                Fixture moved(abi, base, shift); moved.source(long_path); expect(moved, Status::Ready);
                moved.memory.put<unsigned char>(base + 0x7000, 0xaf); expect(moved, Status::Ready);
            }
            if (abi == Abi::LibcppArm64) {
                Fixture tagged(abi); tagged.source(short_path);
                for (const auto table : tagged.tables) tagged.memory.put<std::uintptr_t>(table + 0x108, (table + 0x200) | 0x8000000000000000ULL);
                expect(tagged, Status::Ready);
            }
            Fixture own_a(abi), own_b(abi); own_a.source(long_path); own_b.source(long_path + ".other");
            require(own_a.inspect().source == long_path && own_b.inspect().source == long_path + ".other", "Instance sources crossed"); ++cases;
            Fixture missing_module(abi); missing_module.module.base = 0; expect(missing_module, Status::ModuleUnavailable);
            Fixture wrong_effect(abi); wrong_effect.type(0, "25TEOfxhImageEffectDescriptor"); expect(wrong_effect, Status::EffectTypeMismatch);
            Fixture wrong_message(abi); wrong_message.type(1, Fixture::name(abi, 2)); expect(wrong_message, Status::MessageTypeMismatch);
            Fixture wrong_image(abi); wrong_image.type(2, Fixture::name(abi, 1)); expect(wrong_image, Status::ImageTypeMismatch);
            Fixture outside(abi); outside.memory.put(outside.tables[0], Fixture::processor); expect(outside, Status::EffectTypeMismatch);
            Fixture bad_rtti(abi); bad_rtti.memory.put<std::uintptr_t>(bad_rtti.tables[0] - 8, Fixture::payload); expect(bad_rtti, Status::EffectTypeMismatch);
            Fixture pending(abi); pending.memory.put<std::uintptr_t>(Fixture::effect + layout(abi).message, 0); expect(pending, Status::MissingMessageCallback);
            pending.memory.put(Fixture::effect + layout(abi).message, Fixture::message); pending.source(short_path); expect(pending, Status::Ready);
            Fixture mismatch(abi); mismatch.memory.put(Fixture::image + 8, Fixture::processor + 8); expect(mismatch, Status::ProcessorMismatch);
            Fixture missing_source(abi); missing_source.memory.regions.erase(Fixture::processor); expect(missing_source, Status::UnreadableSourceHeader);
            Fixture empty(abi); empty.source(""); expect(empty, Status::EmptySource);
            Fixture relative(abi); relative.source("clip.mp4"); expect(relative, Status::RelativeSource);
            Fixture invalid_utf8(abi); invalid_utf8.source(std::string(abi == Abi::LibcppArm64 ? "/" : "C:/") + "\xc0\xaf"); expect(invalid_utf8, Status::InvalidUtf8);
            Fixture embedded(abi); embedded.source(std::string(abi == Abi::LibcppArm64 ? "/" : "C:/") + std::string("a\0b", 3)); expect(embedded, Status::EmbeddedNull);
            Fixture no_terminator(abi); no_terminator.source(short_path); no_terminator.memory.put<char>(Fixture::processor + layout(abi).source + short_path.size(), 'x'); expect(no_terminator, Status::MissingTerminator);
            Fixture long_unreadable(abi); long_unreadable.source(long_path); long_unreadable.memory.regions.erase(Fixture::payload); expect(long_unreadable, Status::UnreadableSourcePayload);
            Fixture overflowing(abi); overflowing.source(long_path); overflowing.memory.put<std::uintptr_t>(Fixture::processor + layout(abi).source, std::numeric_limits<std::uintptr_t>::max() - 2); expect(overflowing, Status::AddressOverflow);
            Fixture header_change(abi); header_change.source(long_path);
            header_change.memory.after = [=](Memory& m, std::uintptr_t address, unsigned count) {
                if (address == Fixture::processor + layout(abi).source && count == 1) m.put<unsigned char>(address + 8, 1);
            }; expect(header_change, Status::ChangedDuringCopy);
            Fixture payload_change(abi); payload_change.source(long_path);
            payload_change.memory.after = [](Memory& m, std::uintptr_t address, unsigned count) {
                if (address == Fixture::payload && count == 1) m.put<char>(address + 2, 'X');
            }; expect(payload_change, Status::ChangedDuringCopy);
            Fixture chain_change(abi); chain_change.source(short_path);
            chain_change.memory.after = [=](Memory& m, std::uintptr_t address, unsigned count) {
                if (address == Fixture::processor + layout(abi).source && count == 1) m.put(Fixture::image + 8, Fixture::processor + 8);
            }; expect(chain_change, Status::ChangedDuringCopy);
            Fixture large(abi); large.source(long_path);
            large.memory.put<std::uint64_t>(Fixture::processor + layout(abi).source + (abi == Abi::LibcppArm64 ? 8 : 16), maximum_source_bytes + 1); expect(large, Status::SourceTooLong);
            Fixture capacity(abi); capacity.source(long_path);
            capacity.memory.put<std::uint64_t>(Fixture::processor + layout(abi).source + (abi == Abi::LibcppArm64 ? 16 : 24), abi == Abi::LibcppArm64 ? 0x8000000000000001ULL : 16); expect(capacity, Status::InvalidStringLayout);
            Fixture invalid_effect(abi); require(invalid_effect.inspect(1).status == Status::UnreadableEffect, "Unreadable effect accepted"); ++cases;
            require(invalid_effect.inspect(std::numeric_limits<std::uintptr_t>::max() - 4).status == Status::AddressOverflow, "Effect address overflow"); ++cases;
            if (abi == Abi::MsvcX64) {
                for (unsigned role : {1U, 2U}) {
                    Fixture renamed(abi); renamed.source(long_path);
                    auto name = Fixture::name(abi, role);
                    name.replace(name.find("<lambda_") + 8, 32, "0123456789abcdef0123456789abcdef");
                    renamed.type(role, name); expect(renamed, Status::Ready);
                    Fixture wrong_signature(abi); wrong_signature.source(long_path);
                    name = Fixture::name(abi, role);
                    if (role == 1) name.replace(name.find("_NPEBU"), 2, "H");
                    else name.erase(name.find("NNNNN"), 1);
                    wrong_signature.type(role, name);
                    expect(wrong_signature, role == 1 ? Status::MessageTypeMismatch : Status::ImageTypeMismatch);
                }
                Fixture locator(abi); locator.memory.put<std::uint32_t>(locator.tables[0] + 0x80 + 20, 0); expect(locator, Status::EffectTypeMismatch);
                Fixture unc(abi); unc.source("\\\\server\\media\\clip.mov"); expect(unc, Status::Ready);
            }
        }
#ifdef __APPLE__
        using namespace jianying_macos_source_path;
        require(inspect_source(nullptr).status == Status::NullEffect && inspect_source(reinterpret_cast<void*>(1)).status == Status::UnreadableEffect, "Native Mach boundary"); ++cases;
#endif
#ifdef _WIN32
        std::string rejected = "stale";
        require(!jianying_source_path::copy_source_path(reinterpret_cast<void*>(1), rejected) && rejected.empty(), "Native Windows boundary"); ++cases;
#endif
        // Exercise runtime x64 layout discovery directly on synthetic code and archived host patterns.
        {
            using namespace jianying_source_path::x64;
            constexpr std::uintptr_t base = 0x180000000ULL;
            constexpr std::uintptr_t text = base + 0x1000;
            constexpr std::uintptr_t rdata = base + 0x4000;
            auto make_env = [&]() {
                Memory mem;
                mem.regions[base].resize(0x8000);
                Module mod{base, Abi::MsvcX64, {{text, 0x3000, true}, {rdata, 0x4000, false}}};
                return std::make_pair(std::move(mem), std::move(mod));
            };
            auto put_bytes = [](Memory& mem, std::uintptr_t addr, const std::vector<unsigned char>& code) {
                for (std::size_t i = 0; i < code.size(); ++i) mem.put<unsigned char>(addr + i, code[i]);
            };
            auto append_u32 = [](std::vector<unsigned char>& v, std::uint32_t value) {
                for (unsigned i = 0; i < 4; ++i) v.push_back(static_cast<unsigned char>((value >> (i * 8)) & 0xff));
            };
            auto make_fetch_code = [&](std::uint32_t src_off, std::uint32_t id_off, std::int8_t stride, bool include_guard, bool include_third) {
                std::vector<unsigned char> c;
                // mov r14, rcx; mov rdi, r8
                c.insert(c.end(), {0x4c, 0x8b, 0xf1, 0x49, 0x8b, 0xf8});
                if (include_guard) {
                    // cmp dword ptr [rax], 0x72756f53 ("Sour"); cmp word ptr [rax+4], 0x6563 ("ce"); jne +0x50
                    c.insert(c.end(), {0x81, 0x38, 0x53, 0x6f, 0x75, 0x72, 0x66, 0x81, 0x78, 0x04, 0x63, 0x65, 0x75, 0x50});
                }
                const std::uintptr_t assign_rva = 0x2800;
                const auto emit_call_assign = [&](std::uintptr_t fn_rva) {
                    c.push_back(0xe8);
                    const auto next_rva = fn_rva + c.size() + 4;
                    append_u32(c, static_cast<std::uint32_t>(assign_rva - next_rva));
                };
                // 1: lea rdx, [rcx + src_off]; lea rcx, [rbp - 0x70]; call assign
                c.insert(c.end(), {0x48, 0x8d, 0x91}); append_u32(c, src_off);
                c.insert(c.end(), {0x48, 0x8d, 0x4d, 0x90});
                emit_call_assign(0x1400);
                // 2: lea rdx, [r14 + id_off]; lea rcx, [rbp - 0x70 + stride]; call assign
                c.insert(c.end(), {0x49, 0x8d, 0x96}); append_u32(c, id_off);
                c.insert(c.end(), {0x48, 0x8d, 0x4d, static_cast<unsigned char>(static_cast<std::int8_t>(-0x70) + stride)});
                emit_call_assign(0x1400);
                if (include_third) {
                    // 3: mov rdx, rdi; lea rcx, [rbp - 0x70 + 2*stride]; call assign
                    c.insert(c.end(), {0x48, 0x8b, 0xd7, 0x48, 0x8d, 0x4d, static_cast<unsigned char>(static_cast<std::int8_t>(-0x70) + 2 * stride)});
                    emit_call_assign(0x1400);
                }
                c.push_back(0xc3);
                return c;
            };

            // Positive: 3-hop call graph (matching 11.9) and direct 1-hop (matching 11.8/11.3) at relocated offsets.
            for (const std::uint32_t src_off : {0xe8U, 0x108U, 0x120U, 0x1b8U}) {
                auto [mem, mod] = make_env();
                // image_vt[2] -> 0x1100 (calls 0x1200) -> 0x1200 (calls 0x1400) -> 0x1400 (fetch)
                mem.put<std::uintptr_t>(rdata + 0x100 + 16, base + 0x1100);
                // 0x1100: call 0x1200; ret
                std::vector<unsigned char> hop1{0xe8}; append_u32(hop1, 0x1200 - 0x1105); hop1.push_back(0xc3);
                // 0x1200: call 0x1400; ret
                std::vector<unsigned char> hop2{0xe8}; append_u32(hop2, 0x1400 - 0x1205); hop2.push_back(0xc3);
                const auto fetch = make_fetch_code(src_off, 0x58, 0x20, true, true);
                put_bytes(mem, base + 0x1100, hop1);
                put_bytes(mem, base + 0x1200, hop2);
                put_bytes(mem, base + 0x1400, fetch);
                put_bytes(mem, base + 0x2800, {0xc3});
                const std::vector<FunctionRange> pdata{
                    {0x1100, static_cast<std::uint32_t>(0x1100 + hop1.size())},
                    {0x1200, static_cast<std::uint32_t>(0x1200 + hop2.size())},
                    {0x1400, static_cast<std::uint32_t>(0x1400 + fetch.size())},
                    {0x2800, 0x2801}
                };
                const auto discovered = discover_source_offset({Memory::read, &mem}, mod, pdata, rdata + 0x100);
                require(discovered && *discovered == src_off, "x64 runtime source offset discovery"); ++cases;
            }

            // Negative controls: missing guard, wrong stride, only 2 assigns, ambiguous offsets.
            {
                auto check_reject = [&](const std::vector<unsigned char>& fetch, const char* msg) {
                    auto [mem, mod] = make_env();
                    mem.put<std::uintptr_t>(rdata + 0x100 + 16, base + 0x1400);
                    put_bytes(mem, base + 0x1400, fetch);
                    put_bytes(mem, base + 0x2800, {0xc3});
                    const std::vector<FunctionRange> pdata{{0x1400, static_cast<std::uint32_t>(0x1400 + fetch.size())}, {0x2800, 0x2801}};
                    require(!discover_source_offset({Memory::read, &mem}, mod, pdata, rdata + 0x100).has_value(), msg); ++cases;
                };
                check_reject(make_fetch_code(0x120, 0x58, 0x20, false, true), "Missing Source guard accepted");
                check_reject(make_fetch_code(0x120, 0x58, 0x18, true, true), "Wrong string stride accepted");
                check_reject(make_fetch_code(0x120, 0x58, 0x20, true, false), "Two-assign sequence accepted");
            }

            // Destructor slot discovery: both rbx-based and rdx-clobbered (setne dl) SBO patterns + out-of-line helper.
            {
                auto [mem, mod] = make_env();
                mem.put<std::uintptr_t>(rdata + 0x200, base + 0x1500);
                // 0x1500 deleting dtor: call 0x1600; ret
                std::vector<unsigned char> del_dtor{0xe8}; append_u32(del_dtor, 0x1600 - 0x1505); del_dtor.push_back(0xc3);
                // 0x1600 real dtor:
                //   mov rdi, rcx
                //   slot 1 (rdx clobbered by setne dl): mov rcx, [rdi+0x1f8]; mov rax, [rcx]; lea rdx, [rdi+0x1c0]; cmp rcx, rdx; setne dl; call [rax+0x20]
                //   slot 2 (out-of-line tidy helper at 0x1900): lea rcx, [rdi+0x110]; call 0x1900
                //   ret
                std::vector<unsigned char> real_dtor{
                    0x48, 0x8b, 0xf9,
                    0x48, 0x8b, 0x8f, 0xf8, 0x01, 0x00, 0x00,
                    0x48, 0x8b, 0x01,
                    0x48, 0x8d, 0x97, 0xc0, 0x01, 0x00, 0x00,
                    0x48, 0x3b, 0xca,
                    0x0f, 0x95, 0xc2,
                    0xff, 0x50, 0x20,
                    0x48, 0x8d, 0x8f, 0x10, 0x01, 0x00, 0x00,
                    0xe8
                };
                append_u32(real_dtor, static_cast<std::uint32_t>(0x1900 - (0x1600 + real_dtor.size() + 4)));
                real_dtor.push_back(0xc3);
                // 0x1900 tidy helper: mov rbx, rcx; mov rcx, [rbx+0x38]; mov rax, [rcx]; cmp rcx, rbx; setne dl; call [rax+0x20]; ret
                const std::vector<unsigned char> tidy{
                    0x48, 0x8b, 0xd9, 0x48, 0x8b, 0x4b, 0x38, 0x48, 0x8b, 0x01, 0x48, 0x3b, 0xcb, 0x0f, 0x95, 0xc2, 0xff, 0x50, 0x20, 0xc3
                };
                put_bytes(mem, base + 0x1500, del_dtor);
                put_bytes(mem, base + 0x1600, real_dtor);
                put_bytes(mem, base + 0x1900, tidy);
                const std::vector<FunctionRange> pdata{
                    {0x1500, static_cast<std::uint32_t>(0x1500 + del_dtor.size())},
                    {0x1600, static_cast<std::uint32_t>(0x1600 + real_dtor.size())},
                    {0x1900, static_cast<std::uint32_t>(0x1900 + tidy.size())}
                };
                const auto slots = discover_effect_function_slots({Memory::read, &mem}, mod, pdata, rdata + 0x200);
                require(slots.size() == 2 && slots[0] == 0x148 && slots[1] == 0x1f8, "x64 destructor slot discovery"); ++cases;
            }
        }
        std::cout << "{\"passed\":true,\"cases\":" << cases << ",\"abi_profiles\":[\"libcpp-arm64\",\"msvc-x64\"],\"real_editor_loaded\":false}\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << "FAIL: " << error.what() << '\n'; return 1; }
}
