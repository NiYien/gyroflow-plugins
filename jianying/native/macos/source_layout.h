// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "../source_path_contract.h"
#include <optional>

namespace jianying_macos_source_path::arm64 {
using jianying_source_contract::Layout;
using Instruction = std::uint32_t;
constexpr unsigned sp = 31;
constexpr std::size_t code_words = 96;
using Code = std::array<Instruction, code_words>;

inline unsigned dest(Instruction code) { return code & 31; }
inline unsigned base(Instruction code) { return (code >> 5) & 31; }
inline unsigned right(Instruction code) { return (code >> 16) & 31; }
inline unsigned immediate(Instruction code) { return (code >> 10) & 0xfff; }
inline unsigned memory_offset(Instruction code) { return immediate(code) * 8; }
inline bool add(Instruction code) { return (code & 0xffc00000) == 0x91000000; }
inline bool load(Instruction code) { return (code & 0xffc00000) == 0xf9400000; }
inline bool store(Instruction code) { return (code & 0xffc00000) == 0xf9000000; }
inline bool move(Instruction code, unsigned from) {
    return (code & 0xffe0ffe0) == 0xaa0003e0 && right(code) == from;
}
inline bool compare(Instruction code, bool word = false) {
    return (code & 0xffe0fc1f) == (word ? 0x6b00001f : 0xeb00001f);
}
inline bool branch_if(Instruction code, unsigned condition) {
    return (code & 0xff00001f) == (0x54000000 | condition);
}
inline bool moved_from(const Code& code, std::size_t end, unsigned to, unsigned from) {
    return std::any_of(code.begin(), code.begin() + end,
        [=](Instruction word) { return move(word, from) && dest(word) == to; });
}

inline std::optional<std::int64_t> call_target(Instruction code, std::size_t position) {
    if ((code & 0xfc000000) != 0x94000000) return std::nullopt;
    const auto imm26 = std::int64_t(code & 0x03ffffff);
    const auto displacement = imm26 >= 0x02000000 ? imm26 - 0x04000000 : imm26;
    return static_cast<std::int64_t>(position) + displacement;
}

inline bool source_guard(const Instruction* code) {
    // Match the host's six-byte "Source" comparison, with arbitrary register allocation.
    constexpr Instruction move_immediate = 0xffe00000;
    return (code[0] & move_immediate) == 0x52800000 && ((code[0] >> 5) & 0xffff) == 0x6f53
        && (code[1] & move_immediate) == 0x72a00000 && ((code[1] >> 5) & 0xffff) == 0x7275
        && dest(code[0]) == dest(code[1]) && compare(code[2], true) && right(code[2]) == dest(code[0])
        && (code[3] & move_immediate) == 0x52800000 && ((code[3] >> 5) & 0xffff) == 0x6563
        && (code[4] & 0xffe0fc1f) == 0x7a400000 && right(code[4]) == dest(code[3])
        && branch_if(code[5], 1);
}

inline std::optional<std::size_t> source_member(const Code& code) {
    std::optional<std::size_t> result;
    for (std::size_t guard = 0; guard + 6 <= code.size(); ++guard) {
        if (!source_guard(&code[guard])) continue;
        // The successful branch builds a frame request by assigning three consecutive
        // strings. Take the source of the first assignment, from the processor object.
        for (auto i = guard + 6; i + 9 <= code.size() && i < guard + 34; ++i) {
            const auto* op = &code[i];
            if (!add(op[0]) || dest(op[0]) != 0 || base(op[0]) != sp
                || !add(op[1]) || dest(op[1]) != 1 || base(op[1]) == sp
                || !add(op[3]) || dest(op[3]) != 0 || !add(op[4]) || dest(op[4]) != 1
                || !add(op[6]) || dest(op[6]) != 0) continue;
            const auto target = call_target(op[2], i + 2);
            if (!target || target != call_target(op[5], i + 5) || target != call_target(op[8], i + 8)) continue;
            const auto member = immediate(op[1]);
            const auto request = base(op[3]);
            const auto stride = immediate(op[3]);
            if (!member || member % 8 || base(op[1]) != base(op[4]) || member == immediate(op[4])
                || !stride || base(op[6]) != request || immediate(op[6]) != 2 * stride
                || !move(op[7], right(op[7])) || dest(op[7]) != 1
                || !moved_from(code, guard, base(op[1]), 0)) continue;
            const auto stack_alias = std::any_of(code.begin() + guard + 6, code.begin() + i,
                [=](Instruction word) { return add(word) && dest(word) == request && base(word) == sp
                    && immediate(word) == immediate(op[0]); });
            if (!stack_alias) continue;
            if (result) return std::nullopt;
            result = member;
        }
    }
    return result;
}

inline std::optional<std::size_t> image_member(const Code& code) {
    std::optional<std::size_t> result;
    for (std::size_t i = 0; i + 5 <= code.size(); ++i) {
        const auto* op = &code[i];
        // populate() loads and clears the previous image callable and compares it to
        // its inline storage before disposing of it.
        if (!add(op[0]) || base(op[0]) != 0 || !load(op[1]) || base(op[1]) != 0 || dest(op[1]) != 0
            || !store(op[2]) || dest(op[2]) != 31 || !moved_from(code, i, base(op[2]), 0)
            || memory_offset(op[1]) != memory_offset(op[2]) || immediate(op[0]) >= memory_offset(op[1])
            || !compare(op[3]) || base(op[3]) != 0 || right(op[3]) != dest(op[0])
            || !branch_if(op[4], 0)) continue;
        if (result) return std::nullopt;
        result = memory_offset(op[1]);
    }
    return result;
}

inline std::optional<std::size_t> message_member(const Code& code) {
    std::optional<std::size_t> result;
    for (std::size_t i = 0; i + 3 <= code.size(); ++i) {
        const auto* op = &code[i];
        // The setter loads the effect and the incoming std::function's callable.
        if (!load(op[0]) || base(op[0]) != 0 || dest(op[0]) == 0
            || !load(op[1]) || base(op[1]) != 1 || dest(op[1]) != 0 || !memory_offset(op[1])
            || (op[2] & 0xff00001f) != 0xb4000000) continue;
        const auto effect = dest(op[0]);
        const auto callable = memory_offset(op[1]);
        for (auto j = i + 3; j + 12 <= code.size() && j < i + 40; ++j) {
            if (!add(code[j]) || base(code[j]) != effect) continue;
            const auto member = immediate(code[j]) + callable;
            const auto* swap = &code[j + 4];
            // The move branch swaps the incoming callable with the effect's slot.
            if (!load(swap[0]) || base(swap[0]) != sp || dest(swap[0]) != 0 || memory_offset(swap[0]) != callable
                || !load(swap[1]) || base(swap[1]) != effect || memory_offset(swap[1]) != member
                || !compare(swap[4]) || base(swap[4]) != dest(swap[1]) || right(swap[4]) != dest(code[j])
                || !store(swap[6]) || base(swap[6]) != sp || dest(swap[6]) != dest(swap[1])
                || memory_offset(swap[6]) != callable || !store(swap[7]) || base(swap[7]) != effect
                || dest(swap[7]) != 0 || memory_offset(swap[7]) != member) continue;
            if (result) return std::nullopt;
            result = member;
        }
    }
    return result;
}

inline std::optional<Layout> discover(const Code& source, const Code& image, const Code& message) {
    const auto source_offset = source_member(source);
    const auto image_offset = image_member(image);
    const auto message_offset = message_member(message);
    if (!source_offset || !image_offset || !message_offset || *image_offset == *message_offset) return std::nullopt;
    return Layout{*message_offset, *image_offset, *source_offset};
}
} // namespace jianying_macos_source_path::arm64
