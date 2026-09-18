//! Printable runs in an image.
//!
//! The dump covers the whole file rather than one section, because a format
//! string is the only anchor an instruction refers to and the anchors are
//! scattered. Runs from `.text` are noise in the same table, which is the price
//! of not deciding in advance where a string may live.

/// Shortest run the dump records.
const MIN_RUN: usize = 5;

/// One printable run.
pub struct Run<'a> {
    /// File offset of the first byte.
    pub offset: usize,
    /// The bytes, all of them printable ASCII.
    pub text: &'a [u8],
}

/// Every maximal run of printable ASCII at least [`MIN_RUN`] bytes long.
pub fn scan(bytes: &[u8]) -> Vec<Run<'_>> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, byte) in bytes.iter().enumerate() {
        let printable = (0x20..=0x7e).contains(byte);
        match (printable, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if index - from >= MIN_RUN {
                    out.push(Run {
                        offset: from,
                        text: &bytes[from..index],
                    });
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start
        && bytes.len() - from >= MIN_RUN
    {
        out.push(Run {
            offset: from,
            text: &bytes[from..],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::scan;

    /// One scan input and the runs it is expected to yield.
    type Case = (&'static [u8], &'static [(usize, &'static str)]);

    #[test]
    fn only_maximal_runs_of_five_or_more_printable_bytes_are_recorded() {
        let cases: &[Case] = &[
            (b"", &[]),
            (b"abcd", &[]),
            (b"abcde", &[(0, "abcde")]),
            (b"\0abcde\0", &[(1, "abcde")]),
            (b"abcd\0efghij", &[(5, "efghij")]),
            (b".text\0\0\0`.rdata", &[(0, ".text"), (8, "`.rdata")]),
            (b"one\x01twelve\x7f", &[(4, "twelve")]),
            (b"tail run", &[(0, "tail run")]),
        ];
        for (input, want) in cases {
            let got: Vec<(usize, String)> = scan(input)
                .into_iter()
                .map(|run| (run.offset, String::from_utf8_lossy(run.text).into_owned()))
                .collect();
            let want: Vec<(usize, String)> = want
                .iter()
                .map(|(offset, text)| (*offset, (*text).to_string()))
                .collect();
            assert_eq!(got, want, "input {input:?}");
        }
    }

    #[test]
    fn the_run_boundary_is_the_printable_ascii_range() {
        assert_eq!(scan(b"\x1fabcde\x80").len(), 1);
        assert_eq!(scan(&[0x20; 5]).len(), 1, "space is printable");
        assert_eq!(scan(&[0x7e; 5]).len(), 1, "tilde is printable");
        assert_eq!(scan(&[0x7f; 5]).len(), 0, "delete is not");
    }
}
