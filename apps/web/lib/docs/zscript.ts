import type { LanguageRegistration } from "shiki";

/**
 * A TextMate grammar for Zcash / Bitcoin script as the docs write it: opcodes (`OP_CHECKSIG`), `<placeholders>` for
 * pushed values, numbers, and `#` comments. Fence tag: ```` ```zscript ```` (also `script`, `btcscript`).
 *
 * Opcodes are split by role so the theme can weigh them: flow control recedes, hashing and signature checks stand
 * out, stack operations sit in between.
 */
export const ZSCRIPT_GRAMMAR: LanguageRegistration = {
  name: "zscript",
  scopeName: "source.zscript",
  displayName: "Zcash script",
  aliases: ["script", "btcscript", "bitcoin-script"],
  patterns: [{ include: "#comment" }, { include: "#placeholder" }, { include: "#flow" }, { include: "#check" }, { include: "#opcode" }, { include: "#number" }],
  repository: {
    comment: {
      match: "(#).*$",
      name: "comment.line.number-sign.zscript",
      captures: { 1: { name: "punctuation.definition.comment.zscript" } },
    },
    placeholder: {
      match: "(<)([^<>\\s]+)(>)",
      captures: {
        1: { name: "punctuation.definition.placeholder.begin.zscript" },
        2: { name: "variable.parameter.placeholder.zscript" },
        3: { name: "punctuation.definition.placeholder.end.zscript" },
      },
    },
    flow: {
      match: "\\bOP_(?:IF|NOTIF|ELSE|ENDIF|VERIFY|RETURN|NOP)\\b",
      name: "keyword.control.flow.zscript",
    },
    check: {
      match:
        "\\bOP_(?:SHA256|SHA1|RIPEMD160|HASH160|HASH256|CHECKSIG|CHECKSIGVERIFY|CHECKMULTISIG|CHECKMULTISIGVERIFY|CHECKLOCKTIMEVERIFY|CHECKSEQUENCEVERIFY|EQUALVERIFY|EQUAL)\\b",
      name: "support.function.check.zscript",
    },
    opcode: {
      match: "\\bOP_[A-Z0-9_]+\\b",
      name: "keyword.operator.opcode.zscript",
    },
    number: {
      match: "(?<![\\w<])(?:0x[0-9a-fA-F]+|-?\\d+)(?![\\w>])",
      name: "constant.numeric.zscript",
    },
  },
};
