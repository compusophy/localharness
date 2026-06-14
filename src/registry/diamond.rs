//! Pure ABI encoder for EIP-2535 `diamondCut(...)` calldata.
//!
//! This is the hardest encoder in the repo (design/soliditylite.md §6): the
//! argument is `(FacetCut[], address, bytes)` where `FacetCut[]` is a dynamic
//! array of tuples each of which is ITSELF dynamic (it holds a dynamic
//! `bytes4[]`). Getting it right means threading three nested head/tail offset
//! regions — the outer call, the array's element-offset table, and each
//! tuple's internal `bytes4[]` offset — by hand.
//!
//! Native-testable, no DOM, no state, no async. Golden-vector tested against
//! `cast calldata` output (see the test module).

use super::abi::selector;
use crate::encoding::bytes_to_hex;

/// The 32-byte ABI word size.
const WORD: usize = 32;

/// One `FacetCut` — `{ address facetAddress; uint8 action; bytes4[] selectors }`.
///
/// `action` follows `IDiamond.FacetCutAction`: `0 = Add`, `1 = Replace`,
/// `2 = Remove`. It's a plain `u8` so the encoder doesn't depend on an enum
/// definition (the on-chain ABI just sees a `uint8`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCut {
    /// The facet contract address (20 bytes).
    pub facet: [u8; 20],
    /// The `FacetCutAction`: `0 = Add`, `1 = Replace`, `2 = Remove`.
    pub action: u8,
    /// The 4-byte function selectors this cut Adds/Replaces/Removes.
    pub selectors: Vec<[u8; 4]>,
}

/// A 32-byte big-endian word holding `value` in its low 8 bytes (right-aligned,
/// the Solidity layout for any small scalar — offsets, lengths, `uint8`).
fn word_usize(value: usize) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&(value as u64).to_be_bytes());
    out
}

/// A 32-byte word holding a 20-byte `address` right-aligned (left-padded with
/// 12 zero bytes — the Solidity ABI layout for `address`).
fn word_address(addr: &[u8; 20]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(addr);
    out
}

/// ABI-encode ONE `FacetCut` tuple as a self-contained dynamic blob.
///
/// Tuple layout (the tuple is dynamic because `bytes4[]` is dynamic):
///   word 0: `facetAddress`
///   word 1: `action`
///   word 2: offset (within THIS tuple's encoding) to the `bytes4[]` head
///   then the `bytes4[]`: length word, then one word per selector
///           (each `bytes4` is LEFT-aligned, i.e. right-padded with 28 zeros).
///
/// The selector array always starts right after the 3 head words, so its
/// offset is a constant `3 * 32 = 0x60`.
fn encode_facet_cut(cut: &FacetCut) -> Vec<u8> {
    let mut buf = Vec::with_capacity(WORD * (3 + 1 + cut.selectors.len()));
    // Head.
    buf.extend_from_slice(&word_address(&cut.facet));
    buf.extend_from_slice(&word_usize(cut.action as usize));
    buf.extend_from_slice(&word_usize(3 * WORD)); // offset to bytes4[] = 0x60
    // Tail: the bytes4[] (length + each selector left-aligned in its word).
    buf.extend_from_slice(&word_usize(cut.selectors.len()));
    for sel in &cut.selectors {
        let mut w = [0u8; 32];
        w[..4].copy_from_slice(sel); // bytes4 is LEFT-aligned
        buf.extend_from_slice(&w);
    }
    buf
}

/// ABI-encode the dynamic `FacetCut[]` array (WITHOUT its own outer offset).
///
/// Layout: `length`, then one offset per element (each relative to the START of
/// this array's data area, i.e. just after the length word), then every
/// element's tuple encoding concatenated.
fn encode_facet_cut_array(cuts: &[FacetCut]) -> Vec<u8> {
    let elems: Vec<Vec<u8>> = cuts.iter().map(encode_facet_cut).collect();

    // The element-offset table is `cuts.len()` words; offsets are measured from
    // the start of the data area (the first byte AFTER the length word — equiv.
    // the first byte of the offset table itself).
    let table_bytes = cuts.len() * WORD;
    let mut offsets = Vec::with_capacity(cuts.len());
    let mut running = table_bytes;
    for e in &elems {
        offsets.push(running);
        running += e.len();
    }

    let mut buf = Vec::with_capacity(WORD + table_bytes + running);
    buf.extend_from_slice(&word_usize(cuts.len())); // array length
    for off in offsets {
        buf.extend_from_slice(&word_usize(off));
    }
    for e in elems {
        buf.extend_from_slice(&e);
    }
    buf
}

/// ABI-encode a dynamic `bytes` value (WITHOUT its own outer offset): a length
/// word followed by the data right-padded to a 32-byte multiple.
fn encode_bytes(data: &[u8]) -> Vec<u8> {
    let padded = data.len().div_ceil(WORD) * WORD;
    let mut buf = Vec::with_capacity(WORD + padded);
    buf.extend_from_slice(&word_usize(data.len()));
    buf.extend_from_slice(data);
    buf.resize(WORD + padded, 0);
    buf
}

/// Encode the full `diamondCut((address,uint8,bytes4[])[],address,bytes)`
/// calldata: the 4-byte selector followed by the ABI-encoded
/// `(cuts, init, init_calldata)`.
///
/// The outer arg tuple has three head words — offset-to-`cuts`, the static
/// `init` address, and offset-to-`init_calldata` — followed by the two dynamic
/// tails (the `FacetCut[]` then the `bytes`). Returns raw calldata bytes; use
/// [`encode_diamond_cut_hex`] for the `0x…` string form.
pub fn encode_diamond_cut(cuts: &[FacetCut], init: &[u8; 20], init_calldata: &[u8]) -> Vec<u8> {
    let sel = selector("diamondCut((address,uint8,bytes4[])[],address,bytes)");

    let cuts_blob = encode_facet_cut_array(cuts);
    let calldata_blob = encode_bytes(init_calldata);

    // Three outer head words. The two dynamic args sit in the tail, in order:
    // first the cuts array, then the bytes. Offsets are measured from the start
    // of the argument region (just AFTER the selector).
    const HEAD: usize = 3 * WORD; // offset-cuts, init, offset-calldata
    let cuts_offset = HEAD;
    let calldata_offset = HEAD + cuts_blob.len();

    let mut buf = Vec::with_capacity(4 + HEAD + cuts_blob.len() + calldata_blob.len());
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&word_usize(cuts_offset)); // offset to FacetCut[]
    buf.extend_from_slice(&word_address(init)); // init address (static)
    buf.extend_from_slice(&word_usize(calldata_offset)); // offset to bytes
    buf.extend_from_slice(&cuts_blob);
    buf.extend_from_slice(&calldata_blob);
    buf
}

/// Like [`encode_diamond_cut`] but returns a `0x`-prefixed lowercase hex string
/// (the shape `eth_sendRawTransaction` / tx-`input` assembly expect).
pub fn encode_diamond_cut_hex(cuts: &[FacetCut], init: &[u8; 20], init_calldata: &[u8]) -> String {
    format!("0x{}", bytes_to_hex(&encode_diamond_cut(cuts, init, init_calldata)))
}

/// ABI-encode the `Diamond` constructor's argument tuple
/// `(address _contractOwner, FacetCut[] _diamondCut)` — NO function selector.
///
/// Constructor args are appended raw to the creation bytecode (there is no
/// 4-byte selector for a constructor), so this returns just the encoded tuple.
/// Use it when assembling child-diamond genesis bytecode
/// (`Diamond` creation code ‖ these args) for a `CREATE`/`CREATE2` deploy.
///
/// CRITICAL: the arg order is `(owner, cuts)` — the OPPOSITE of
/// [`encode_diamond_cut`]'s `diamondCut(cuts, init, calldata)`. The head is two
/// words — the left-padded `owner` address (static) then the offset to the
/// dynamic `FacetCut[]` (a constant `0x40`, since the head is exactly two
/// words) — followed by the identical `FacetCut[]` tail produced by
/// [`encode_facet_cut_array`] (length, element-offset table, each tuple with its
/// inner `bytes4[]`).
pub fn encode_diamond_constructor_args(owner: &[u8; 20], cuts: &[FacetCut]) -> Vec<u8> {
    let cuts_blob = encode_facet_cut_array(cuts);

    // Two outer head words: the static `owner` address then the offset to the
    // (only) dynamic arg. The cuts array begins right after the head, so its
    // offset is a constant `2 * 32 = 0x40`.
    const HEAD: usize = 2 * WORD; // owner, offset-cuts
    let mut buf = Vec::with_capacity(HEAD + cuts_blob.len());
    buf.extend_from_slice(&word_address(owner)); // owner address (static)
    buf.extend_from_slice(&word_usize(HEAD)); // offset to FacetCut[] = 0x40
    buf.extend_from_slice(&cuts_blob);
    buf
}

/// Like [`encode_diamond_constructor_args`] but returns a `0x`-prefixed
/// lowercase hex string.
pub fn encode_diamond_constructor_args_hex(owner: &[u8; 20], cuts: &[FacetCut]) -> String {
    format!("0x{}", bytes_to_hex(&encode_diamond_constructor_args(owner, cuts)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selector for the diamondCut signature — `cast sig` cross-check.
    /// `cast sig "diamondCut((address,uint8,bytes4[])[],address,bytes)"` →
    /// `0x1f931c1c`.
    #[test]
    fn selector_matches_cast() {
        let sel = selector("diamondCut((address,uint8,bytes4[])[],address,bytes)");
        assert_eq!(bytes_to_hex(&sel), "1f931c1c");
    }

    /// GOLDEN VECTOR 1 — one Add cut, two selectors, zero init, empty calldata.
    ///
    /// Derived with foundry `cast` (v1.6.0-nightly-tempo):
    /// ```text
    /// cast calldata "diamondCut((address,uint8,bytes4[])[],address,bytes)" \
    ///   "[(0x1111111111111111111111111111111111111111,0,[0xaabbccdd,0x11223344])]" \
    ///   0x0000000000000000000000000000000000000000 0x
    /// ```
    #[test]
    fn golden_add_two_selectors() {
        let expected = concat!(
            "0x1f931c1c",
            // outer head: offset-cuts(0x60), init(0x0), offset-calldata(0x160)
            "0000000000000000000000000000000000000000000000000000000000000060",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000160",
            // FacetCut[]: length 1
            "0000000000000000000000000000000000000000000000000000000000000001",
            // element[0] offset (from start of array data) = 0x20
            "0000000000000000000000000000000000000000000000000000000000000020",
            // tuple: facetAddress
            "0000000000000000000000001111111111111111111111111111111111111111",
            // tuple: action (Add = 0)
            "0000000000000000000000000000000000000000000000000000000000000000",
            // tuple: offset to bytes4[] within tuple = 0x60
            "0000000000000000000000000000000000000000000000000000000000000060",
            // bytes4[]: length 2
            "0000000000000000000000000000000000000000000000000000000000000002",
            // selector 0xaabbccdd (left-aligned)
            "aabbccdd00000000000000000000000000000000000000000000000000000000",
            // selector 0x11223344 (left-aligned)
            "1122334400000000000000000000000000000000000000000000000000000000",
            // trailing bytes: length 0
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let cut = FacetCut {
            facet: [0x11; 20],
            action: 0,
            selectors: vec![[0xaa, 0xbb, 0xcc, 0xdd], [0x11, 0x22, 0x33, 0x44]],
        };
        let got = encode_diamond_cut_hex(&[cut], &[0u8; 20], &[]);
        assert_eq!(got, expected);
    }

    /// GOLDEN VECTOR 2 — one Remove cut, one selector, NON-ZERO init
    /// (`0x…beef`), short 2-byte calldata (`0xcafe`). Exercises a non-zero
    /// static address word and a non-empty (padded) trailing `bytes`.
    ///
    /// Derived with foundry `cast`:
    /// ```text
    /// cast calldata "diamondCut((address,uint8,bytes4[])[],address,bytes)" \
    ///   "[(0x2222222222222222222222222222222222222222,2,[0xdeadbeef])]" \
    ///   0x000000000000000000000000000000000000beef 0xcafe
    /// ```
    #[test]
    fn golden_remove_nonzero_init_short_calldata() {
        let expected = concat!(
            "0x1f931c1c",
            // outer head: offset-cuts(0x60), init(0x…beef), offset-calldata(0x140)
            "0000000000000000000000000000000000000000000000000000000000000060",
            "000000000000000000000000000000000000000000000000000000000000beef",
            "0000000000000000000000000000000000000000000000000000000000000140",
            // FacetCut[]: length 1
            "0000000000000000000000000000000000000000000000000000000000000001",
            // element[0] offset = 0x20
            "0000000000000000000000000000000000000000000000000000000000000020",
            // tuple: facetAddress 0x2222…2222
            "0000000000000000000000002222222222222222222222222222222222222222",
            // tuple: action (Remove = 2)
            "0000000000000000000000000000000000000000000000000000000000000002",
            // tuple: offset to bytes4[] = 0x60
            "0000000000000000000000000000000000000000000000000000000000000060",
            // bytes4[]: length 1
            "0000000000000000000000000000000000000000000000000000000000000001",
            // selector 0xdeadbeef (left-aligned)
            "deadbeef00000000000000000000000000000000000000000000000000000000",
            // trailing bytes: length 2, data 0xcafe right-padded
            "0000000000000000000000000000000000000000000000000000000000000002",
            "cafe000000000000000000000000000000000000000000000000000000000000",
        );
        let mut init = [0u8; 20];
        init[18] = 0xbe;
        init[19] = 0xef;
        let cut = FacetCut {
            facet: [0x22; 20],
            action: 2,
            selectors: vec![[0xde, 0xad, 0xbe, 0xef]],
        };
        let got = encode_diamond_cut_hex(&[cut], &init, &[0xca, 0xfe]);
        assert_eq!(got, expected);
    }

    /// Sanity: the raw-bytes form equals the hex form modulo the `0x` prefix,
    /// and the calldata is always a whole number of 32-byte words past the
    /// 4-byte selector.
    #[test]
    fn raw_and_hex_agree_and_word_aligned() {
        let cut = FacetCut {
            facet: [0x11; 20],
            action: 0,
            selectors: vec![[0xaa, 0xbb, 0xcc, 0xdd]],
        };
        let raw = encode_diamond_cut(std::slice::from_ref(&cut), &[0u8; 20], &[]);
        let hex = encode_diamond_cut_hex(&[cut], &[0u8; 20], &[]);
        assert_eq!(hex, format!("0x{}", bytes_to_hex(&raw)));
        assert_eq!((raw.len() - 4) % WORD, 0);
    }

    /// Empty cuts list: the array degrades to a lone length-0 word, and the
    /// trailing bytes offset shifts accordingly. Mirrors
    /// `cast calldata … "[]" 0x…0 0x` (offset-cuts 0x60, then length 0, then
    /// offset-calldata 0x80).
    #[test]
    fn empty_cuts_list() {
        let got = encode_diamond_cut_hex(&[], &[0u8; 20], &[]);
        let expected = concat!(
            "0x1f931c1c",
            "0000000000000000000000000000000000000000000000000000000000000060",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000080",
            // FacetCut[]: length 0 (no offset table, no elements)
            "0000000000000000000000000000000000000000000000000000000000000000",
            // trailing bytes: length 0
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert_eq!(got, expected);
    }

    /// GOLDEN VECTOR — `Diamond` constructor args `(address, FacetCut[])`, ONE
    /// Add cut with two selectors. Note the arg ORDER is `(owner, cuts)` (owner
    /// FIRST), the OPPOSITE of `diamondCut(cuts, init, calldata)`, and there is
    /// NO leading 4-byte selector (constructor args append raw to the bytecode).
    ///
    /// Derived with foundry `cast` (v1.6.0-nightly-tempo):
    /// ```text
    /// cast abi-encode "constructor(address,(address,uint8,bytes4[])[])" \
    ///   0x1111111111111111111111111111111111111111 \
    ///   "[(0x2222222222222222222222222222222222222222,0,[0xaabbccdd,0x11223344])]"
    /// ```
    #[test]
    fn golden_constructor_args_one_cut() {
        let expected = concat!(
            "0x",
            // head: owner (left-padded address)
            "0000000000000000000000001111111111111111111111111111111111111111",
            // head: offset to FacetCut[] = 0x40 (head is exactly two words)
            "0000000000000000000000000000000000000000000000000000000000000040",
            // FacetCut[]: length 1
            "0000000000000000000000000000000000000000000000000000000000000001",
            // element[0] offset (from start of array data) = 0x20
            "0000000000000000000000000000000000000000000000000000000000000020",
            // tuple: facetAddress 0x2222…2222
            "0000000000000000000000002222222222222222222222222222222222222222",
            // tuple: action (Add = 0)
            "0000000000000000000000000000000000000000000000000000000000000000",
            // tuple: offset to bytes4[] within tuple = 0x60
            "0000000000000000000000000000000000000000000000000000000000000060",
            // bytes4[]: length 2
            "0000000000000000000000000000000000000000000000000000000000000002",
            // selector 0xaabbccdd (left-aligned)
            "aabbccdd00000000000000000000000000000000000000000000000000000000",
            // selector 0x11223344 (left-aligned)
            "1122334400000000000000000000000000000000000000000000000000000000",
        );
        let cut = FacetCut {
            facet: [0x22; 20],
            action: 0,
            selectors: vec![[0xaa, 0xbb, 0xcc, 0xdd], [0x11, 0x22, 0x33, 0x44]],
        };
        let got = encode_diamond_constructor_args_hex(&[0x11; 20], std::slice::from_ref(&cut));
        assert_eq!(got, expected);

        // Raw form agrees with the hex form, and is whole 32-byte words (no
        // selector to offset by, unlike diamondCut calldata).
        let raw = encode_diamond_constructor_args(&[0x11; 20], std::slice::from_ref(&cut));
        assert_eq!(got, format!("0x{}", bytes_to_hex(&raw)));
        assert_eq!(raw.len() % WORD, 0);
    }

    /// GOLDEN VECTOR — TWO cuts (Add facet `0x44…44` with one selector, then
    /// Remove facet `0x55…55` with two selectors). Exercises the element-offset
    /// table with a non-trivial second offset (`0xe0`) and a Remove action.
    ///
    /// Derived with foundry `cast`:
    /// ```text
    /// cast abi-encode "constructor(address,(address,uint8,bytes4[])[])" \
    ///   0x3333333333333333333333333333333333333333 \
    ///   "[(0x4444444444444444444444444444444444444444,0,[0xdeadbeef]),\
    ///     (0x5555555555555555555555555555555555555555,2,[0xcafebabe,0xfeedface])]"
    /// ```
    #[test]
    fn golden_constructor_args_two_cuts_remove() {
        let expected = concat!(
            "0x",
            // head: owner 0x3333…3333
            "0000000000000000000000003333333333333333333333333333333333333333",
            // head: offset to FacetCut[] = 0x40
            "0000000000000000000000000000000000000000000000000000000000000040",
            // FacetCut[]: length 2
            "0000000000000000000000000000000000000000000000000000000000000002",
            // element[0] offset = 0x40 (just past the 2-word offset table)
            "0000000000000000000000000000000000000000000000000000000000000040",
            // element[1] offset = 0xe0 (after element[0]: 5 words = 0xa0, +0x40)
            "00000000000000000000000000000000000000000000000000000000000000e0",
            // element[0] tuple: facetAddress 0x4444…4444
            "0000000000000000000000004444444444444444444444444444444444444444",
            // element[0]: action (Add = 0)
            "0000000000000000000000000000000000000000000000000000000000000000",
            // element[0]: offset to bytes4[] = 0x60
            "0000000000000000000000000000000000000000000000000000000000000060",
            // element[0] bytes4[]: length 1
            "0000000000000000000000000000000000000000000000000000000000000001",
            // selector 0xdeadbeef (left-aligned)
            "deadbeef00000000000000000000000000000000000000000000000000000000",
            // element[1] tuple: facetAddress 0x5555…5555
            "0000000000000000000000005555555555555555555555555555555555555555",
            // element[1]: action (Remove = 2)
            "0000000000000000000000000000000000000000000000000000000000000002",
            // element[1]: offset to bytes4[] = 0x60
            "0000000000000000000000000000000000000000000000000000000000000060",
            // element[1] bytes4[]: length 2
            "0000000000000000000000000000000000000000000000000000000000000002",
            // selector 0xcafebabe (left-aligned)
            "cafebabe00000000000000000000000000000000000000000000000000000000",
            // selector 0xfeedface (left-aligned)
            "feedface00000000000000000000000000000000000000000000000000000000",
        );
        let cuts = [
            FacetCut {
                facet: [0x44; 20],
                action: 0,
                selectors: vec![[0xde, 0xad, 0xbe, 0xef]],
            },
            FacetCut {
                facet: [0x55; 20],
                action: 2,
                selectors: vec![[0xca, 0xfe, 0xba, 0xbe], [0xfe, 0xed, 0xfa, 0xce]],
            },
        ];
        let got = encode_diamond_constructor_args_hex(&[0x33; 20], &cuts);
        assert_eq!(got, expected);
    }
}
