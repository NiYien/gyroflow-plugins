// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "source_path_contract.h"
#include <array>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <set>
#include <utility>
#include <vector>

namespace jianying_source_path::x64 {
using jianying_source_contract::Layout;
using jianying_source_contract::Module;
using jianying_source_contract::Reader;
using jianying_source_contract::Role;
using jianying_source_contract::matches_type;
using jianying_source_contract::safe_add;

constexpr std::uint8_t reg_none = 0xff;
constexpr std::uint8_t reg_rax = 0, reg_rcx = 1, reg_rdx = 2, reg_rbx = 3;
constexpr std::uint8_t reg_rsp = 4, reg_rbp = 5, reg_rsi = 6, reg_rdi = 7;
constexpr std::uint8_t reg_r8 = 8, reg_r9 = 9, reg_r10 = 10, reg_r11 = 11;
constexpr std::uint16_t volatile_reg_mask =
    (1u << reg_rax) | (1u << reg_rcx) | (1u << reg_rdx)
    | (1u << reg_r8) | (1u << reg_r9) | (1u << reg_r10) | (1u << reg_r11);

constexpr std::size_t max_function_bytes = 8192;
constexpr std::size_t max_dtor_depth = 2;
constexpr std::size_t max_dtor_nodes = 8;
constexpr std::size_t max_source_bfs_depth = 4;
constexpr std::size_t max_source_bfs_nodes = 24;
constexpr std::size_t max_effect_member_offset = 0x4000;
constexpr std::size_t max_source_member_offset = 0x4000;
constexpr std::size_t msvc_string_size = 32;
constexpr std::size_t msvc_function_impl_offset = 0x38;

struct FunctionRange {
    std::uint32_t begin = 0;
    std::uint32_t end = 0;
};

struct Instruction {
    enum class Kind {
        Other,
        CallRel,
        CallMem,
        JmpRel,
        JccRel,
        Ret,
        Lea,
        MovRegReg,
        MovRegMem,
        AddRegImm,
        TestRegReg,
        CmpRegReg,
        CmpMemImm,
        SetneReg
    };

    std::uintptr_t address = 0;
    std::size_t length = 0;
    Kind kind = Kind::Other;
    std::uint8_t dst = reg_none;
    std::uint8_t src = reg_none;
    std::uint8_t base = reg_none;
    std::uint8_t index = reg_none;
    std::uint8_t op_size = 0;
    std::uint8_t cc = 0;
    std::int64_t disp = 0;
    std::uint64_t imm = 0;
    std::uintptr_t target = 0;
    std::uint16_t clobbers = 0;
};

inline std::uint16_t read_u16(const unsigned char* p) {
    return static_cast<std::uint16_t>(p[0]) | (static_cast<std::uint16_t>(p[1]) << 8);
}

inline std::uint32_t read_u32(const unsigned char* p) {
    return static_cast<std::uint32_t>(p[0])
        | (static_cast<std::uint32_t>(p[1]) << 8)
        | (static_cast<std::uint32_t>(p[2]) << 16)
        | (static_cast<std::uint32_t>(p[3]) << 24);
}

inline std::uint64_t read_u64(const unsigned char* p) {
    return static_cast<std::uint64_t>(read_u32(p))
        | (static_cast<std::uint64_t>(read_u32(p + 4)) << 32);
}

inline std::int64_t sign_extend(std::uint64_t value, unsigned bits) {
    const auto shift = 64 - bits;
    return static_cast<std::int64_t>(value << shift) >> shift;
}

inline bool relative_target(std::uintptr_t next_ip, std::int64_t rel, std::uintptr_t& out) {
    if (rel >= 0) return safe_add(next_ip, static_cast<std::uint64_t>(rel), out);
    const auto magnitude = static_cast<std::uint64_t>(-(rel + 1)) + 1;
    if (next_ip < magnitude) return false;
    out = next_ip - static_cast<std::uintptr_t>(magnitude);
    return true;
}

// Decode one x86-64 instruction, returning false on truncated or unsupported encodings.
inline bool decode(const unsigned char* bytes, std::size_t size, std::uintptr_t address, Instruction& out) {
    out = {};
    out.address = address;
    if (!bytes || !size) return false;

    std::size_t pos = 0;
    bool p66 = false, p67 = false, pf2 = false, pf3 = false;
    bool rex_w = false, rex_r = false, rex_x = false, rex_b = false;
    bool has_vex = false;

    while (pos < size && pos < 15) {
        const auto b = bytes[pos];
        if (b == 0x66) { p66 = true; ++pos; }
        else if (b == 0x67) { p67 = true; ++pos; }
        else if (b == 0xF2) { pf2 = true; ++pos; }
        else if (b == 0xF3) { pf3 = true; ++pos; }
        else if (b == 0xF0 || b == 0x2E || b == 0x36 || b == 0x3E || b == 0x26 || b == 0x64 || b == 0x65) { ++pos; }
        else break;
    }
    if (pos >= size) return false;

    std::uint8_t map = 0; // 0 = 1-byte, 1 = 0F, 2 = 0F 38, 3 = 0F 3A
    if ((bytes[pos] & 0xF0) == 0x40) {
        const auto rex = bytes[pos++];
        rex_w = (rex & 0x08) != 0;
        rex_r = (rex & 0x04) != 0;
        rex_x = (rex & 0x02) != 0;
        rex_b = (rex & 0x01) != 0;
        if (pos >= size) return false;
    } else if (bytes[pos] == 0xC5 && !p66 && !pf2 && !pf3) {
        if (pos + 2 >= size) return false;
        const auto v1 = bytes[pos + 1];
        rex_r = (v1 & 0x80) == 0;
        const auto pp = v1 & 3;
        p66 = (pp == 1); pf3 = (pp == 2); pf2 = (pp == 3);
        map = 1;
        has_vex = true;
        pos += 2;
    } else if (bytes[pos] == 0xC4 && !p66 && !pf2 && !pf3) {
        if (pos + 3 >= size) return false;
        const auto v1 = bytes[pos + 1];
        const auto v2 = bytes[pos + 2];
        rex_r = (v1 & 0x80) == 0;
        rex_x = (v1 & 0x40) == 0;
        rex_b = (v1 & 0x20) == 0;
        map = v1 & 0x1F;
        if (map < 1 || map > 3) return false;
        rex_w = (v2 & 0x80) != 0;
        const auto pp = v2 & 3;
        p66 = (pp == 1); pf3 = (pp == 2); pf2 = (pp == 3);
        has_vex = true;
        pos += 3;
    }

    if (pos >= size) return false;
    std::uint8_t op = bytes[pos++];
    if (!has_vex && op == 0x0F) {
        if (pos >= size) return false;
        op = bytes[pos++];
        if (op == 0x38) {
            if (pos >= size) return false;
            op = bytes[pos++];
            map = 2;
        } else if (op == 0x3A) {
            if (pos >= size) return false;
            op = bytes[pos++];
            map = 3;
        } else {
            map = 1;
        }
    }

    const std::size_t iz = (!rex_w && p66) ? 2 : 4;
    bool has_modrm = false;
    std::size_t imm_bytes = 0;
    bool imm_from_f6 = false, imm_from_f7 = false;

    if (map == 0) {
        if ((op <= 0x3F && (op & 7) <= 3)) { has_modrm = true; }
        else if (op <= 0x3F && (op & 7) == 4) { imm_bytes = 1; }
        else if (op <= 0x3F && (op & 7) == 5) { imm_bytes = iz; }
        else if (op >= 0x50 && op <= 0x5F) {}
        else if (op == 0x63) { has_modrm = true; }
        else if (op == 0x68) { imm_bytes = iz; }
        else if (op == 0x69) { has_modrm = true; imm_bytes = iz; }
        else if (op == 0x6A) { imm_bytes = 1; }
        else if (op == 0x6B) { has_modrm = true; imm_bytes = 1; }
        else if (op >= 0x6C && op <= 0x6F) {}
        else if (op >= 0x70 && op <= 0x7F) { imm_bytes = 1; }
        else if (op == 0x80 || op == 0x83) { has_modrm = true; imm_bytes = 1; }
        else if (op == 0x81) { has_modrm = true; imm_bytes = iz; }
        else if (op >= 0x84 && op <= 0x8F) { has_modrm = true; }
        else if (op >= 0x90 && op <= 0x99) {}
        else if (op >= 0x9B && op <= 0x9F) {}
        else if (op >= 0xA0 && op <= 0xA3) { imm_bytes = p67 ? 4 : 8; }
        else if ((op >= 0xA4 && op <= 0xA7) || (op >= 0xAA && op <= 0xAF)) {}
        else if (op == 0xA8) { imm_bytes = 1; }
        else if (op == 0xA9) { imm_bytes = iz; }
        else if (op >= 0xB0 && op <= 0xB7) { imm_bytes = 1; }
        else if (op >= 0xB8 && op <= 0xBF) { imm_bytes = rex_w ? 8 : iz; }
        else if (op == 0xC0 || op == 0xC1 || op == 0xC6) { has_modrm = true; imm_bytes = 1; }
        else if (op == 0xC2 || op == 0xCA) { imm_bytes = 2; }
        else if (op == 0xC3 || op == 0xC9 || op == 0xCB || op == 0xCC || op == 0xCF) {}
        else if (op == 0xC7) { has_modrm = true; imm_bytes = iz; }
        else if (op == 0xC8) { imm_bytes = 3; }
        else if (op == 0xCD) { imm_bytes = 1; }
        else if (op >= 0xD0 && op <= 0xD3) { has_modrm = true; }
        else if (op == 0xD7) {}
        else if (op >= 0xD8 && op <= 0xDF) { has_modrm = true; }
        else if ((op >= 0xE0 && op <= 0xE7) || op == 0xEB) { imm_bytes = 1; }
        else if (op == 0xE8 || op == 0xE9) { imm_bytes = 4; }
        else if (op >= 0xEC && op <= 0xEF) {}
        else if (op == 0xF1 || op == 0xF4 || op == 0xF5 || (op >= 0xF8 && op <= 0xFD)) {}
        else if (op == 0xF6) { has_modrm = true; imm_from_f6 = true; }
        else if (op == 0xF7) { has_modrm = true; imm_from_f7 = true; }
        else if (op == 0xFE || op == 0xFF) { has_modrm = true; }
        else return false;
    } else if (map == 1) {
        if (op >= 0x80 && op <= 0x8F) { imm_bytes = 4; }
        else if (op == 0x05 || op == 0x06 || op == 0x07 || op == 0x08 || op == 0x09 || op == 0x0B
            || op == 0x0E || (op >= 0x30 && op <= 0x35) || op == 0x37 || op == 0x77
            || op == 0xA0 || op == 0xA1 || op == 0xA2 || op == 0xA8 || op == 0xA9 || op == 0xAA
            || (op >= 0xC8 && op <= 0xCF)) {}
        else if (op == 0x70 || op == 0x71 || op == 0x72 || op == 0x73 || op == 0xA4 || op == 0xAC
            || op == 0xBA || op == 0xC2 || op == 0xC4 || op == 0xC5 || op == 0xC6) {
            has_modrm = true; imm_bytes = 1;
        } else {
            has_modrm = true;
        }
    } else if (map == 2) {
        has_modrm = true;
    } else if (map == 3) {
        has_modrm = true;
        imm_bytes = 1;
    } else {
        return false;
    }

    std::uint8_t mod = 0, reg_field = 0, rm_reg = reg_none;
    std::uint8_t mem_base = reg_none, mem_index = reg_none;
    bool is_mem = false, is_rip = false;
    std::int64_t disp = 0;

    if (has_modrm) {
        if (p67) return false;
        if (pos >= size) return false;
        const auto modrm = bytes[pos++];
        mod = (modrm >> 6) & 3;
        const std::uint8_t reg_low = (modrm >> 3) & 7;
        const std::uint8_t rm_low = modrm & 7;
        reg_field = static_cast<std::uint8_t>(reg_low | (rex_r ? 8 : 0));
        if (imm_from_f6 && reg_low <= 1) imm_bytes = 1;
        if (imm_from_f7 && reg_low <= 1) imm_bytes = iz;

        if (mod == 3) {
            rm_reg = static_cast<std::uint8_t>(rm_low | (rex_b ? 8 : 0));
        } else {
            is_mem = true;
            std::size_t disp_bytes = 0;
            if (rm_low == 4) {
                if (pos >= size) return false;
                const auto sib = bytes[pos++];
                const std::uint8_t idx_low = (sib >> 3) & 7;
                const std::uint8_t base_low = sib & 7;
                const auto full_index = static_cast<std::uint8_t>(idx_low | (rex_x ? 8 : 0));
                if (full_index != reg_rsp) mem_index = full_index;
                if (mod == 0 && base_low == 5) {
                    disp_bytes = 4;
                } else {
                    mem_base = static_cast<std::uint8_t>(base_low | (rex_b ? 8 : 0));
                    if (mod == 1) disp_bytes = 1;
                    else if (mod == 2) disp_bytes = 4;
                }
            } else if (mod == 0 && rm_low == 5) {
                is_rip = true;
                disp_bytes = 4;
            } else {
                mem_base = static_cast<std::uint8_t>(rm_low | (rex_b ? 8 : 0));
                if (mod == 1) disp_bytes = 1;
                else if (mod == 2) disp_bytes = 4;
            }
            if (disp_bytes > size - pos) return false;
            if (disp_bytes == 1) disp = sign_extend(bytes[pos], 8);
            else if (disp_bytes == 4) disp = sign_extend(read_u32(bytes + pos), 32);
            pos += disp_bytes;
        }
    }

    if (imm_bytes > size - pos || pos + imm_bytes > 15) return false;
    std::uint64_t imm = 0;
    if (imm_bytes == 1) imm = bytes[pos];
    else if (imm_bytes == 2) imm = read_u16(bytes + pos);
    else if (imm_bytes == 4) imm = read_u32(bytes + pos);
    else if (imm_bytes == 8) imm = read_u64(bytes + pos);
    pos += imm_bytes;

    out.length = pos;
    std::uintptr_t next_ip = 0;
    if (!safe_add(address, pos, next_ip)) return false;

    const auto mark = [&](std::uint8_t r) { if (r < 16) out.clobbers |= static_cast<std::uint16_t>(1u << r); };

    if (map == 0) {
        if (op == 0xE8) {
            out.kind = Instruction::Kind::CallRel;
            out.clobbers = volatile_reg_mask;
            if (!relative_target(next_ip, sign_extend(imm, 32), out.target)) return false;
        } else if (op == 0xE9 || op == 0xEB) {
            out.kind = Instruction::Kind::JmpRel;
            const auto rel = op == 0xE9 ? sign_extend(imm, 32) : sign_extend(imm, 8);
            if (!relative_target(next_ip, rel, out.target)) return false;
        } else if (op >= 0x70 && op <= 0x7F) {
            out.kind = Instruction::Kind::JccRel;
            out.cc = op & 0x0F;
            if (!relative_target(next_ip, sign_extend(imm, 8), out.target)) return false;
        } else if (op == 0xC3 || op == 0xC2) {
            out.kind = Instruction::Kind::Ret;
        } else if (op == 0xFF && (reg_field & 7) == 2) {
            out.clobbers = volatile_reg_mask;
            if (is_mem && !is_rip && mem_index == reg_none) {
                out.kind = Instruction::Kind::CallMem;
                out.base = mem_base;
                out.disp = disp;
            }
        } else if (op == 0x8D && rex_w) {
            mark(reg_field);
            if (is_mem && !is_rip && mem_index == reg_none) {
                out.kind = Instruction::Kind::Lea;
                out.dst = reg_field;
                out.base = mem_base;
                out.disp = disp;
            }
        } else if (op == 0x89) {
            if (!is_mem) {
                mark(rm_reg);
                if (rex_w) {
                    out.kind = Instruction::Kind::MovRegReg;
                    out.dst = rm_reg;
                    out.src = reg_field;
                }
            }
        } else if (op == 0x8B) {
            mark(reg_field);
            if (!is_mem && rex_w) {
                out.kind = Instruction::Kind::MovRegReg;
                out.dst = reg_field;
                out.src = rm_reg;
            } else if (is_mem && rex_w && !is_rip && mem_index == reg_none) {
                out.kind = Instruction::Kind::MovRegMem;
                out.dst = reg_field;
                out.base = mem_base;
                out.disp = disp;
            }
        } else if ((op == 0x83 || op == 0x81) && (reg_field & 7) == 0) {
            if (!is_mem) {
                mark(rm_reg);
                if (rex_w) {
                    out.kind = Instruction::Kind::AddRegImm;
                    out.dst = rm_reg;
                    out.imm = static_cast<std::uint64_t>(sign_extend(imm, op == 0x83 ? 8 : static_cast<unsigned>(iz * 8)));
                }
            }
        } else if (op == 0x85 && !is_mem && rex_w) {
            out.kind = Instruction::Kind::TestRegReg;
            out.dst = rm_reg;
            out.src = reg_field;
        } else if ((op == 0x39 || op == 0x3B) && !is_mem && rex_w) {
            out.kind = Instruction::Kind::CmpRegReg;
            out.dst = (op == 0x39) ? rm_reg : reg_field;
            out.src = (op == 0x39) ? reg_field : rm_reg;
        } else if ((op == 0x83 || op == 0x81) && (reg_field & 7) == 7) {
            if (is_mem && !is_rip && mem_index == reg_none) {
                out.kind = Instruction::Kind::CmpMemImm;
                out.base = mem_base;
                out.disp = disp;
                out.op_size = rex_w ? 8 : (p66 ? 2 : 4);
                out.imm = (op == 0x83) ? static_cast<std::uint64_t>(sign_extend(imm, 8)) : imm;
            }
        } else {
            // Track GP register writes for remaining 1-byte opcodes.
            if (op <= 0x37 && (op & 7) <= 3) {
                if ((op & 2) != 0) mark(reg_field);
                else if (!is_mem) mark(rm_reg);
            } else if (op <= 0x35 && ((op & 7) == 4 || (op & 7) == 5)) {
                mark(reg_rax);
            } else if (op >= 0x58 && op <= 0x5F) {
                mark(static_cast<std::uint8_t>((op - 0x58) | (rex_b ? 8 : 0)));
            } else if (op == 0x63 || op == 0x69 || op == 0x6B || op == 0x8A || op == 0x8C || op == 0x8D) {
                mark(reg_field);
            } else if (op == 0x80 || op == 0x81 || op == 0x83) {
                if ((reg_field & 7) != 7 && !is_mem) mark(rm_reg);
            } else if (op == 0x86 || op == 0x87) {
                mark(reg_field);
                if (!is_mem) mark(rm_reg);
            } else if (op == 0x88 || op == 0x8F || op == 0xC0 || op == 0xC1 || op == 0xC6 || op == 0xC7
                || (op >= 0xD0 && op <= 0xD3)) {
                if (!is_mem) mark(rm_reg);
            } else if (op >= 0x91 && op <= 0x97) {
                mark(reg_rax);
                mark(static_cast<std::uint8_t>((op - 0x90) | (rex_b ? 8 : 0)));
            } else if (op == 0x98 || op == 0xA0 || op == 0xA1 || op == 0xD7) {
                mark(reg_rax);
            } else if (op == 0x99) {
                mark(reg_rdx);
            } else if (op >= 0xB0 && op <= 0xBF) {
                mark(static_cast<std::uint8_t>((op & 7) | (rex_b ? 8 : 0)));
            } else if (op == 0xF6 || op == 0xF7) {
                const auto sub = reg_field & 7;
                if ((sub == 2 || sub == 3) && !is_mem) mark(rm_reg);
                else if (sub >= 4) { mark(reg_rax); mark(reg_rdx); }
            } else if ((op == 0xFE || op == 0xFF) && (reg_field & 7) <= 1 && !is_mem) {
                mark(rm_reg);
            }
        }
    } else if (map == 1 && !has_vex) {
        if (op >= 0x80 && op <= 0x8F) {
            out.kind = Instruction::Kind::JccRel;
            out.cc = op & 0x0F;
            if (!relative_target(next_ip, sign_extend(imm, 32), out.target)) return false;
        } else if (op == 0x95 && !is_mem) {
            out.kind = Instruction::Kind::SetneReg;
            out.dst = rm_reg;
            mark(rm_reg);
        } else if (op >= 0x90 && op <= 0x9F) {
            if (!is_mem) mark(rm_reg);
        } else if ((op >= 0x40 && op <= 0x4F) || op == 0xAF || op == 0xB6 || op == 0xB7
            || op == 0xBC || op == 0xBD || op == 0xBE || op == 0xBF || op == 0x2C || op == 0x2D
            || op == 0x50 || op == 0xC5 || op == 0xD7) {
            mark(reg_field);
        } else if (op == 0x7E && p66 && !is_mem) {
            mark(rm_reg);
        } else if (op == 0xB0 || op == 0xB1) {
            mark(reg_rax);
            if (!is_mem) mark(rm_reg);
        } else if (op == 0xC0 || op == 0xC1) {
            mark(reg_field);
            if (!is_mem) mark(rm_reg);
        } else if (op >= 0xC8 && op <= 0xCF) {
            mark(static_cast<std::uint8_t>((op - 0xC8) | (rex_b ? 8 : 0)));
        } else if (op == 0xA2) {
            mark(reg_rax); mark(reg_rbx); mark(reg_rcx); mark(reg_rdx);
        } else if (op == 0xA4 || op == 0xA5 || op == 0xAC || op == 0xAD || op == 0xAB || op == 0xB3
            || op == 0xBB || (op == 0xBA && (reg_field & 7) >= 5)) {
            if (!is_mem) mark(rm_reg);
        }
    } else if (has_vex) {
        if (map == 1 && (op == 0x2C || op == 0x2D || op == 0x50 || op == 0xC5 || op == 0xD7)) mark(reg_field);
        else if (map == 1 && op == 0x7E && p66 && !is_mem) mark(rm_reg);
    }

    return true;
}

inline const FunctionRange* find_pdata_entry(const Module& module, const std::vector<FunctionRange>& pdata,
    std::uintptr_t address) {
    if (!module.base || address < module.base || pdata.empty()) return nullptr;
    const auto rva = address - module.base;
    if (rva > std::numeric_limits<std::uint32_t>::max()) return nullptr;
    std::size_t lo = 0, hi = pdata.size();
    while (lo < hi) {
        const auto mid = lo + (hi - lo) / 2;
        if (rva < pdata[mid].begin) hi = mid;
        else if (rva >= pdata[mid].end) lo = mid + 1;
        else return &pdata[mid];
    }
    return nullptr;
}

inline bool decode_function(const Reader& reader, const Module& module, const std::vector<FunctionRange>& pdata,
    std::uintptr_t entry, std::uintptr_t& begin_out, std::uintptr_t& end_out, std::vector<Instruction>& insns) {
    insns.clear();
    if (!module.contains(entry, 1, true)) return false;

    const auto* pdata_entry = find_pdata_entry(module, pdata, entry);
    std::size_t size = 0;
    if (pdata_entry) {
        if (pdata_entry->end <= pdata_entry->begin) return false;
        size = static_cast<std::size_t>(pdata_entry->end - pdata_entry->begin);
        if (size > max_function_bytes) return false;
        if (!safe_add(module.base, pdata_entry->begin, begin_out) || !module.contains(begin_out, size, true)) return false;
    } else {
        // Leaf functions and small thunks may not have a .pdata entry.
        const auto avail = module.available(entry, true);
        if (!avail) return false;
        begin_out = entry;
        size = std::min<std::size_t>(avail, 128);
    }
    if (!safe_add(begin_out, size, end_out) || entry < begin_out || entry >= end_out) return false;

    std::vector<unsigned char> code(size);
    if (!reader.read(begin_out, code.data(), size)) return false;

    std::size_t offset = static_cast<std::size_t>(entry - begin_out);
    while (offset < size) {
        if (!pdata_entry && code[offset] == 0xCC) break;
        Instruction insn;
        std::uintptr_t ip = 0;
        if (!safe_add(begin_out, offset, ip)) return false;
        if (!decode(code.data() + offset, size - offset, ip, insn)) break;
        insns.push_back(insn);
        offset += insn.length;
        if (!pdata_entry && (insn.kind == Instruction::Kind::Ret
            || (insn.kind == Instruction::Kind::JmpRel && (insn.target < begin_out || insn.target >= end_out)))) {
            break;
        }
    }
    return !insns.empty();
}

// Check whether a helper function implements MSVC std::function::_Tidy on its rcx parameter.
inline bool is_std_function_tidy_helper(const Reader& reader, const Module& module,
    const std::vector<FunctionRange>& pdata, std::uintptr_t address) {
    std::uintptr_t begin = 0, end = 0;
    std::vector<Instruction> insns;
    if (!decode_function(reader, module, pdata, address, begin, end, insns)) return false;
    if (insns.size() > 40) return false;

    std::uint16_t self_regs = static_cast<std::uint16_t>(1u << reg_rcx);
    std::optional<std::size_t> last_load_impl;
    std::optional<std::size_t> last_sbo_cmp;
    for (std::size_t i = 0; i < insns.size(); ++i) {
        const auto& insn = insns[i];
        if (insn.kind == Instruction::Kind::MovRegMem && insn.dst == reg_rcx
            && insn.base < 16 && (self_regs & (1u << insn.base))
            && insn.disp == static_cast<std::int64_t>(msvc_function_impl_offset)) {
            last_load_impl = i;
        } else if (insn.kind == Instruction::Kind::CmpRegReg
            && ((insn.dst == reg_rcx && insn.src < 16 && (self_regs & (1u << insn.src)))
                || (insn.src == reg_rcx && insn.dst < 16 && (self_regs & (1u << insn.dst))))) {
            last_sbo_cmp = i;
        } else if (insn.kind == Instruction::Kind::CallMem && insn.disp == 0x20) {
            const auto start = i > 10 ? i - 10 : 0;
            bool saw_setne = false;
            for (std::size_t j = start; j < i; ++j) {
                if (insns[j].kind == Instruction::Kind::SetneReg && insns[j].dst == reg_rdx) saw_setne = true;
            }
            if (saw_setne && last_load_impl && last_sbo_cmp && *last_load_impl >= start && *last_sbo_cmp > *last_load_impl) {
                return true;
            }
        }
        if (insn.kind == Instruction::Kind::MovRegReg && insn.dst < 16 && insn.src < 16) {
            if (self_regs & (1u << insn.src)) self_regs |= static_cast<std::uint16_t>(1u << insn.dst);
            else self_regs &= static_cast<std::uint16_t>(~(1u << insn.dst));
        } else {
            self_regs &= static_cast<std::uint16_t>(~insn.clobbers);
        }
    }
    return false;
}

inline void collect_dtor_function_slots(const Reader& reader, const Module& module,
    const std::vector<FunctionRange>& pdata, std::uintptr_t fn_addr, std::size_t depth,
    std::set<std::uintptr_t>& visited, std::set<std::size_t>& slots) {
    if (depth > max_dtor_depth || visited.size() >= max_dtor_nodes || !visited.insert(fn_addr).second) return;
    std::uintptr_t begin = 0, end = 0;
    std::vector<Instruction> insns;
    if (!decode_function(reader, module, pdata, fn_addr, begin, end, insns)) return;

    std::uint16_t this_regs = static_cast<std::uint16_t>(1u << reg_rcx);
    struct MemberPtr { bool valid = false; std::size_t offset = 0; };
    std::array<MemberPtr, 16> member_ptrs{};
    struct LoadEvent { std::size_t index = 0; std::size_t impl_offset = 0; };
    struct CmpEvent { std::size_t index = 0; std::size_t base_offset = 0; };
    std::optional<LoadEvent> last_impl_load;
    std::optional<CmpEvent> last_sbo_cmp;
    std::vector<std::uintptr_t> child_dtors;

    auto record_slot = [&](std::size_t impl_offset) {
        if (impl_offset >= msvc_function_impl_offset && impl_offset % 8 == 0 && impl_offset <= max_effect_member_offset) {
            slots.insert(impl_offset);
        }
    };

    for (std::size_t i = 0; i < insns.size(); ++i) {
        const auto& insn = insns[i];
        // Track loads of _Impl from [member_ptr + 0x38] or [this + impl_off] before the SBO comparison.
        if (insn.kind == Instruction::Kind::MovRegMem && insn.dst == reg_rcx && insn.base < 16) {
            if (member_ptrs[insn.base].valid && insn.disp == static_cast<std::int64_t>(msvc_function_impl_offset)) {
                last_impl_load = {i, member_ptrs[insn.base].offset + msvc_function_impl_offset};
            } else if ((this_regs & (1u << insn.base))
                && insn.disp >= static_cast<std::int64_t>(msvc_function_impl_offset)
                && insn.disp % 8 == 0 && static_cast<std::uint64_t>(insn.disp) <= max_effect_member_offset) {
                last_impl_load = {i, static_cast<std::size_t>(insn.disp)};
            }
        }

        // Capture SBO comparison (cmp rcx, &member) before setne dl can overwrite the register.
        if (insn.kind == Instruction::Kind::CmpRegReg) {
            const std::uint8_t other = insn.dst == reg_rcx ? insn.src : (insn.src == reg_rcx ? insn.dst : reg_none);
            if (other < 16 && member_ptrs[other].valid) {
                last_sbo_cmp = {i, member_ptrs[other].offset};
            }
        }

        // Pattern A: inlined MSVC std::function::_Tidy() calling _Impl->_Delete_this(dl).
        if (insn.kind == Instruction::Kind::CallMem && insn.disp == 0x20) {
            const auto start = i > 12 ? i - 12 : 0;
            bool saw_vtable = false, saw_setne = false;
            for (std::size_t j = start; j < i; ++j) {
                const auto& prev = insns[j];
                if (prev.kind == Instruction::Kind::MovRegMem && prev.base == reg_rcx && prev.disp == 0
                    && prev.dst == insn.base) {
                    saw_vtable = true;
                } else if (prev.kind == Instruction::Kind::SetneReg && prev.dst == reg_rdx) {
                    saw_setne = true;
                }
            }
            if (saw_vtable && saw_setne && last_impl_load && last_sbo_cmp
                && last_impl_load->index >= start && last_sbo_cmp->index > last_impl_load->index
                && last_sbo_cmp->base_offset + msvc_function_impl_offset == last_impl_load->impl_offset) {
                record_slot(last_impl_load->impl_offset);
            }
        }

        // Pattern B: out-of-line std::function::_Tidy() helper called with rcx = &this->member.
        if (insn.kind == Instruction::Kind::CallRel && member_ptrs[reg_rcx].valid
            && (insn.target < begin || insn.target >= end) && module.contains(insn.target, 1, true)) {
            if (is_std_function_tidy_helper(reader, module, pdata, insn.target)) {
                record_slot(member_ptrs[reg_rcx].offset + msvc_function_impl_offset);
            }
        }

        // Record direct calls or tail jumps passing `this` in rcx (e.g. deleting dtor -> real dtor -> base dtor).
        if ((insn.kind == Instruction::Kind::CallRel || insn.kind == Instruction::Kind::JmpRel)
            && (this_regs & (1u << reg_rcx))
            && (insn.target < begin || insn.target >= end) && module.contains(insn.target, 1, true)) {
            child_dtors.push_back(insn.target);
        }

        // Update tracked register values after inspecting the instruction.
        if (insn.kind == Instruction::Kind::MovRegReg && insn.dst < 16 && insn.src < 16) {
            if (this_regs & (1u << insn.src)) this_regs |= static_cast<std::uint16_t>(1u << insn.dst);
            else this_regs &= static_cast<std::uint16_t>(~(1u << insn.dst));
            member_ptrs[insn.dst] = member_ptrs[insn.src];
        } else if (insn.kind == Instruction::Kind::Lea && insn.dst < 16) {
            this_regs &= static_cast<std::uint16_t>(~(1u << insn.dst));
            if (insn.base < 16 && (this_regs & (1u << insn.base))
                && insn.disp > 0 && insn.disp % 8 == 0 && static_cast<std::uint64_t>(insn.disp) <= max_effect_member_offset) {
                member_ptrs[insn.dst] = {true, static_cast<std::size_t>(insn.disp)};
            } else {
                member_ptrs[insn.dst] = {};
            }
        } else if (insn.kind == Instruction::Kind::AddRegImm && insn.dst < 16) {
            const auto delta = static_cast<std::int64_t>(insn.imm);
            if ((this_regs & (1u << insn.dst)) && delta > 0 && delta % 8 == 0
                && static_cast<std::uint64_t>(delta) <= max_effect_member_offset) {
                this_regs &= static_cast<std::uint16_t>(~(1u << insn.dst));
                member_ptrs[insn.dst] = {true, static_cast<std::size_t>(delta)};
            } else {
                this_regs &= static_cast<std::uint16_t>(~(1u << insn.dst));
                member_ptrs[insn.dst] = {};
            }
        } else {
            this_regs &= static_cast<std::uint16_t>(~insn.clobbers);
            for (std::uint8_t r = 0; r < 16; ++r) {
                if (insn.clobbers & (1u << r)) member_ptrs[r] = {};
            }
        }
    }

    for (const auto child : child_dtors) {
        collect_dtor_function_slots(reader, module, pdata, child, depth + 1, visited, slots);
    }
}

// Discover candidate std::function _Impl pointer offsets from TEOfxhImageEffectInstance's destructor.
inline std::vector<std::size_t> discover_effect_function_slots(const Reader& reader, const Module& module,
    const std::vector<FunctionRange>& pdata, std::uintptr_t effect_vtable) {
    std::uintptr_t deleting_dtor = 0;
    if (!module.read(reader, effect_vtable, deleting_dtor) || !module.contains(deleting_dtor, 1, true)) return {};

    std::set<std::uintptr_t> visited;
    std::set<std::size_t> slots;
    collect_dtor_function_slots(reader, module, pdata, deleting_dtor, 0, visited, slots);
    return std::vector<std::size_t>(slots.begin(), slots.end());
}

// Extract the processor source-path offset from a decoded fetchFrameFromEditor body.
inline std::optional<std::size_t> match_source_offset_in_function(const std::vector<Instruction>& insns) {
    struct Origin {
        enum class Kind { Unknown, ProcBase, ProcField, ClipArg, StackAddr } kind = Kind::Unknown;
        std::uint8_t base = reg_none;
        std::int64_t offset = 0;
    };
    struct StringCall {
        std::uintptr_t target = 0;
        Origin dst;
        Origin src;
    };

    const auto step_regs = [](std::array<Origin, 16>& state, const Instruction& insn) {
        if (insn.kind == Instruction::Kind::MovRegReg && insn.dst < 16 && insn.src < 16) {
            state[insn.dst] = state[insn.src];
        } else if (insn.kind == Instruction::Kind::Lea && insn.dst < 16) {
            if (insn.base == reg_rbp || insn.base == reg_rsp) {
                state[insn.dst] = {Origin::Kind::StackAddr, insn.base, insn.disp};
            } else if (insn.base < 16 && state[insn.base].kind == Origin::Kind::ProcBase
                && insn.disp > 0 && insn.disp % 8 == 0 && static_cast<std::uint64_t>(insn.disp) <= max_source_member_offset) {
                state[insn.dst] = {Origin::Kind::ProcField, reg_none, insn.disp};
            } else if (insn.base < 16 && state[insn.base].kind == Origin::Kind::StackAddr) {
                state[insn.dst] = {Origin::Kind::StackAddr, state[insn.base].base, state[insn.base].offset + insn.disp};
            } else {
                state[insn.dst] = {};
            }
        } else if (insn.kind == Instruction::Kind::MovRegMem && insn.dst < 16) {
            // Preserve origin across MSVC SSO heap-buffer dereference: mov reg, qword ptr [reg].
            if (insn.base < 16 && insn.disp == 0
                && (state[insn.base].kind == Origin::Kind::ProcField || state[insn.base].kind == Origin::Kind::ClipArg)) {
                state[insn.dst] = state[insn.base];
            } else {
                state[insn.dst] = {};
            }
        } else if (insn.kind == Instruction::Kind::AddRegImm && insn.dst < 16) {
            const auto delta = static_cast<std::int64_t>(insn.imm);
            if (state[insn.dst].kind == Origin::Kind::ProcBase && delta > 0 && delta % 8 == 0
                && static_cast<std::uint64_t>(delta) <= max_source_member_offset) {
                state[insn.dst] = {Origin::Kind::ProcField, reg_none, delta};
            } else if (state[insn.dst].kind == Origin::Kind::StackAddr) {
                state[insn.dst].offset += delta;
            } else {
                state[insn.dst] = {};
            }
        } else {
            for (std::uint8_t r = 0; r < 16; ++r) {
                if (insn.clobbers & (1u << r)) state[r] = {};
            }
        }
    };

    std::array<Origin, 16> regs{};
    regs[reg_rcx] = {Origin::Kind::ProcBase, reg_none, 0};
    regs[reg_r8] = {Origin::Kind::ClipArg, reg_none, 0};

    std::optional<std::size_t> result;
    for (std::size_t i = 0; i < insns.size(); ++i) {
        // Match the six-byte "Source" comparison ("Sour" + "ce") followed by jne.
        if (insns[i].kind == Instruction::Kind::CmpMemImm && insns[i].op_size == 4
            && insns[i].disp == 0 && insns[i].imm == 0x72756f53ULL) {
            std::size_t guard_end = 0;
            for (std::size_t j = i + 1; j < insns.size() && j <= i + 6; ++j) {
                if (insns[j].kind == Instruction::Kind::CmpMemImm && insns[j].op_size == 2
                    && insns[j].disp == 4 && insns[j].imm == 0x6563ULL) {
                    if (j + 1 < insns.size() && insns[j + 1].kind == Instruction::Kind::JccRel && insns[j + 1].cc == 5) {
                        guard_end = j + 2;
                    }
                    break;
                }
            }
            if (guard_end) {
                auto local_regs = regs;
                for (std::size_t k = i; k < guard_end; ++k) step_regs(local_regs, insns[k]);
                std::vector<StringCall> calls;
                const auto limit = std::min(insns.size(), guard_end + 110);
                for (std::size_t k = guard_end; k < limit; ++k) {
                    if (insns[k].kind == Instruction::Kind::CallRel) {
                        calls.push_back({insns[k].target, local_regs[reg_rcx], local_regs[reg_rdx]});
                    }
                    step_regs(local_regs, insns[k]);
                }
                // Match three consecutive assignments to a stack request struct: (source_path, clip_id, clip_name).
                for (std::size_t c = 0; c + 3 <= calls.size(); ++c) {
                    const auto& a = calls[c];
                    const auto& b = calls[c + 1];
                    const auto& d = calls[c + 2];
                    if (a.target != b.target || b.target != d.target) continue;
                    if (a.dst.kind != Origin::Kind::StackAddr || b.dst.kind != Origin::Kind::StackAddr || d.dst.kind != Origin::Kind::StackAddr) continue;
                    if (a.dst.base != b.dst.base || b.dst.base != d.dst.base) continue;
                    if (b.dst.offset - a.dst.offset != static_cast<std::int64_t>(msvc_string_size)
                        || d.dst.offset - b.dst.offset != static_cast<std::int64_t>(msvc_string_size)) continue;
                    if (a.src.kind != Origin::Kind::ProcField || b.src.kind != Origin::Kind::ProcField
                        || d.src.kind != Origin::Kind::ClipArg) continue;
                    if (a.src.offset <= 0 || a.src.offset == b.src.offset) continue;
                    const auto candidate = static_cast<std::size_t>(a.src.offset);
                    if (result && *result != candidate) return std::nullopt;
                    result = candidate;
                }
            }
        }
        step_regs(regs, insns[i]);
    }
    return result;
}

// Discover the processor source-path offset by walking direct calls from the image callback's _Do_call slot.
inline std::optional<std::size_t> discover_source_offset(const Reader& reader, const Module& module,
    const std::vector<FunctionRange>& pdata, std::uintptr_t image_vtable) {
    std::uintptr_t do_call = 0;
    if (!module.read(reader, image_vtable + 16, do_call) || !module.contains(do_call, 1, true)) return std::nullopt;

    std::vector<std::pair<std::uintptr_t, std::size_t>> queue{{do_call, 0}};
    std::set<std::uintptr_t> visited;
    std::optional<std::size_t> found;

    for (std::size_t head = 0; head < queue.size() && visited.size() < max_source_bfs_nodes; ++head) {
        const auto [address, depth] = queue[head];
        if (!visited.insert(address).second) continue;
        std::uintptr_t begin = 0, end = 0;
        std::vector<Instruction> insns;
        if (!decode_function(reader, module, pdata, address, begin, end, insns)) continue;
        if (const auto candidate = match_source_offset_in_function(insns)) {
            if (found && *found != *candidate) return std::nullopt;
            found = *candidate;
        }
        if (depth >= max_source_bfs_depth) continue;
        for (const auto& insn : insns) {
            if ((insn.kind == Instruction::Kind::CallRel || insn.kind == Instruction::Kind::JmpRel)
                && (insn.target < begin || insn.target >= end)
                && module.contains(insn.target, 1, true)
                && !visited.count(insn.target)) {
                queue.push_back({insn.target, depth + 1});
            }
        }
    }
    return found;
}

struct DiscoveredCallbacks {
    jianying_source_contract::Status status = jianying_source_contract::Status::ModuleUnavailable;
    std::size_t message_offset = 0;
    std::size_t image_offset = 0;
    std::uintptr_t image_vtable = 0;
};

// Identify the message and image callback slots among destructor-verified std::function fields.
inline DiscoveredCallbacks identify_callback_slots(const Reader& reader, const Module& module,
    std::uintptr_t effect, const std::vector<std::size_t>& candidate_slots) {
    using jianying_source_contract::Status;
    DiscoveredCallbacks result;
    if (candidate_slots.size() < 2) {
        result.status = Status::ModuleUnavailable;
        return result;
    }

    std::optional<std::size_t> message_slot, image_slot;
    std::uintptr_t image_vt = 0;
    unsigned null_slots = 0;

    for (const auto slot : candidate_slots) {
        std::uintptr_t impl = 0;
        if (!reader.at(effect, slot, impl)) {
            result.status = Status::UnreadableEffect;
            return result;
        }
        if (!impl) {
            ++null_slots;
            continue;
        }
        std::uintptr_t vt = 0;
        if (!reader.at(impl, 0, vt)) continue;
        if (matches_type(reader, module, vt, Role::Image)) {
            if (image_slot) { result.status = Status::ModuleUnavailable; return result; }
            image_slot = slot;
            image_vt = vt;
        } else if (matches_type(reader, module, vt, Role::Message)) {
            if (message_slot) { result.status = Status::ModuleUnavailable; return result; }
            message_slot = slot;
        }
    }

    if (!image_slot) {
        result.status = null_slots ? Status::MissingImageCallback : Status::ImageTypeMismatch;
        return result;
    }
    if (!message_slot) {
        result.status = null_slots ? Status::MissingMessageCallback : Status::MessageTypeMismatch;
        return result;
    }
    if (*message_slot == *image_slot) {
        result.status = Status::ModuleUnavailable;
        return result;
    }

    result.status = Status::Ready;
    result.message_offset = *message_slot;
    result.image_offset = *image_slot;
    result.image_vtable = image_vt;
    return result;
}
} // namespace jianying_source_path::x64
