// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SNOMED CT identifier (SCTID) check-digit validation.
//!
//! Every SCTID's final digit is a check digit computed over the preceding
//! digits using the Verhoeff algorithm (SNOMED CT Technical Implementation
//! Guide, §"SNOMED CT Identifiers"). This module validates that digit; it
//! does not otherwise interpret or parse SCTID structure (namespace,
//! partition-id, item identifier).

// Verhoeff multiplication table `d[i][j]`.
const D: [[u8; 10]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
    [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
    [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
    [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
    [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
    [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
    [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
    [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
    [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
];

// Verhoeff permutation table `p[i][j]`, cycled over `i % 8`.
const P: [[u8; 10]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
    [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
    [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
    [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
    [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
    [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
    [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
];

/// Shortest and longest digit-length an SCTID can plausibly have (item
/// identifier + 2-digit partition-id + 1-digit check digit).
const MIN_LEN: usize = 6;
const MAX_LEN: usize = 18;

/// True if `s` is a syntactically plausible, check-digit-valid SCTID: 6-18
/// ASCII digits whose last digit is the correct Verhoeff check digit over the
/// rest. Does not check that the concept exists in any database.
pub fn is_valid_sctid(s: &str) -> bool {
    if s.len() < MIN_LEN || s.len() > MAX_LEN || !s.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    verhoeff_checksum(s) == 0
}

/// Compute the Verhoeff checksum of a digit string (0 iff valid). Processes
/// digits right-to-left, including the check digit itself as position 0.
fn verhoeff_checksum(digits: &str) -> u8 {
    let mut c: u8 = 0;
    for (i, ch) in digits.bytes().rev().enumerate() {
        let digit = (ch - b'0') as usize;
        c = D[c as usize][P[i % 8][digit] as usize];
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real, well-known production SCTIDs (also present in this repo's
    // synthetic RF2 fixture) - see AGENTS.md's cross-check rule.
    const KNOWN_VALID: &[&str] = &[
        "138875005", // SNOMED CT Concept (root)
        "404684003", // Clinical finding
        "71388002",  // Procedure
        "123037004", // Body structure
        "105590001", // Substance
        "410662002", // Concept model attribute
        "116680003", // Is a
        "363698007", // Finding site
        "116676008", // Associated morphology
        "73211009",  // Diabetes mellitus
        "46635009",  // Type 1 diabetes mellitus
        "44054006",  // Type 2 diabetes mellitus
        "22298006",  // Myocardial infarction
        "195967001", // Asthma
        "74281007",  // Myocardium structure
        "55641003",  // Infarct
        "387517004", // Paracetamol
    ];

    #[test]
    fn known_real_sctids_are_valid() {
        for id in KNOWN_VALID {
            assert!(is_valid_sctid(id), "{id} should be a valid SCTID");
        }
    }

    #[test]
    fn a_single_mistyped_digit_is_detected() {
        for id in KNOWN_VALID {
            let bytes = id.as_bytes();
            for i in 0..bytes.len() {
                let mut mutated = bytes.to_vec();
                let original = mutated[i];
                // Try every other digit at this position; at least one must
                // exist since there are 10 digits and only the original is
                // excluded.
                for d in b'0'..=b'9' {
                    if d == original {
                        continue;
                    }
                    mutated[i] = d;
                    let candidate = String::from_utf8(mutated.clone()).unwrap();
                    // Verhoeff detects all single-digit substitution errors,
                    // so every mutation at every position must be caught.
                    assert!(
                        !is_valid_sctid(&candidate),
                        "{candidate} (from {id}, position {i}) should be invalid"
                    );
                }
            }
        }
    }

    #[test]
    fn transposed_adjacent_digits_are_detected() {
        // Verhoeff detects all transpositions of adjacent digits.
        for id in KNOWN_VALID {
            let bytes = id.as_bytes();
            for i in 0..bytes.len() - 1 {
                if bytes[i] == bytes[i + 1] {
                    continue; // no-op transposition
                }
                let mut mutated = bytes.to_vec();
                mutated.swap(i, i + 1);
                let candidate = String::from_utf8(mutated).unwrap();
                assert!(
                    !is_valid_sctid(&candidate),
                    "{candidate} (transposed from {id} at {i}) should be invalid"
                );
            }
        }
    }

    #[test]
    fn non_digit_characters_are_rejected() {
        assert!(!is_valid_sctid("7321100x"));
        assert!(!is_valid_sctid("73211 09"));
        assert!(!is_valid_sctid(""));
    }

    #[test]
    fn out_of_range_lengths_are_rejected() {
        assert!(!is_valid_sctid("123")); // too short
        assert!(!is_valid_sctid(&"1".repeat(19))); // too long
    }
}
