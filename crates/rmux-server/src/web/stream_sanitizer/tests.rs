use super::*;

fn sanitize(chunks: &[&[u8]]) -> Vec<u8> {
    sanitize_for_role(WebShareConnectRole::Operator, chunks)
}

fn sanitize_for_role(role: WebShareConnectRole, chunks: &[&[u8]]) -> Vec<u8> {
    let mut sanitizer = WebTerminalSanitizer::for_role(role);
    let mut output = Vec::new();
    for chunk in chunks {
        sanitizer.push(chunk, &mut output);
    }
    output
}

#[test]
fn spectator_role_removes_private_metadata_and_keeps_visual_osc() {
    let input = concat!(
        "A\u{1b}]0;private icon and title\u{1b}\\",
        "B\u{1b}]1;private icon\u{1b}\\",
        "C\u{1b}]2;private title\u{1b}\\",
        "D\u{1b}]7;file:///home/owner/private\u{1b}\\",
        "E\u{1b}]133;P;Cwd=/home/owner/private\u{1b}\\",
        "F\u{1b}]4;1;rgb:ff/00/00\u{1b}\\",
        "G\u{1b}]8;;https://example.test\u{1b}\\link\u{1b}]8;;\u{1b}\\H",
    )
    .as_bytes();
    let expected = concat!(
        "ABCDEF",
        "\u{1b}]4;1;rgb:ff/00/00\u{1b}\\",
        "G",
        "\u{1b}]8;;https://example.test\u{1b}\\link\u{1b}]8;;\u{1b}\\H",
    )
    .as_bytes();

    for split in 0..=input.len() {
        assert_eq!(
            sanitize_for_role(
                WebShareConnectRole::Spectator,
                &[&input[..split], &input[split..]],
            ),
            expected,
            "split {split}"
        );
    }
}

#[test]
fn operator_role_preserves_authorized_private_metadata() {
    let input = concat!(
        "A\u{1b}]0;private icon and title\u{1b}\\",
        "B\u{1b}]1;private icon\u{1b}\\",
        "C\u{1b}]2;private title\u{1b}\\",
        "D\u{1b}]7;file:///home/owner/private\u{1b}\\",
        "E\u{1b}]133;P;Cwd=/home/owner/private\u{1b}\\F",
    )
    .as_bytes();

    for split in 0..=input.len() {
        assert_eq!(
            sanitize_for_role(
                WebShareConnectRole::Operator,
                &[&input[..split], &input[split..]],
            ),
            input,
            "split {split}"
        );
    }
}

#[test]
fn osc_52_is_removed_at_every_fragmentation_boundary() {
    let input = b"before\x1b]52;c;Zm9v\x1b\\after";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"beforeafter",
            "split {split}"
        );
    }
}

#[test]
fn allowed_visual_osc_survives_all_fragmentation_boundaries() {
    let input = b"A\x1b]8;;https://example.test\x1b\\link\x1b]8;;\x07B";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            input,
            "split {split}"
        );
    }
}

#[test]
fn osc_8_allows_web_links_and_closures_but_drops_active_content_schemes() {
    for input in [
        b"A\x1b]8;;javascript:alert(1)\x1b\\B".as_slice(),
        b"A\x1b]8;;data:text/html,unsafe\x07B".as_slice(),
        b"A\x1b]8;;file:///etc/passwd\x1b\\B".as_slice(),
        b"A\x1b]8;;relative/path\x1b\\B".as_slice(),
    ] {
        for split in 0..=input.len() {
            assert_eq!(
                sanitize(&[&input[..split], &input[split..]]),
                b"AB",
                "split {split}"
            );
        }
    }
    for input in [
        b"A\x1b]8;;https://example.test\x1b\\B".as_slice(),
        b"A\x1b]8;id=1;MAILTO:user@example.test\x07B".as_slice(),
        b"A\x1b]8;;\x1b\\B".as_slice(),
    ] {
        assert_eq!(sanitize(&[input]), input);
    }
}

#[test]
fn apc_dcs_pm_and_sos_are_explicitly_dropped() {
    let input = b"a\x1b_Gi=1;kitty\x1b\\b\x1bPqSIXEL\x1b\\c\x1b^pm\x1b\\d\x1bXsos\x1b\\e";
    for first in 0..=input.len() {
        for second in first..=input.len() {
            assert_eq!(
                sanitize(&[&input[..first], &input[first..second], &input[second..]]),
                b"abcde",
                "splits {first}/{second}"
            );
        }
    }
}

#[test]
fn bare_c1_bytes_become_viewer_safe_replacements() {
    // RMUX renders each malformed byte as U+FFFD. Pinned xterm.js drops
    // malformed UTF-8, so the boundary emits the replacement explicitly.
    let input = b"a\x9d52;c;Zm9v\x9cb\x9fGkitty\x9cc\x90qsixel\x9cd";
    let expected =
        "a\u{fffd}52;c;Zm9v\u{fffd}b\u{fffd}Gkitty\u{fffd}c\u{fffd}qsixel\u{fffd}d".as_bytes();
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            expected,
            "split {split}"
        );
    }
}

#[test]
fn a_bare_c1_byte_never_swallows_the_rest_of_a_pane_stream() {
    // `cat` of a binary file, then ordinary shell output. Before the fix
    // the viewer received the first chunk's prefix and nothing else.
    let chunks: [&[u8]; 3] = [
        b"$ cat logo.bin\r\n\x9f\x8a\x00PNG",
        b"\r\n$ echo hello\r\nhello\r\n$ ",
        b"date\r\nFri Jul 25 12:00:00 CEST 2026\r\n$ ",
    ];
    let raw = chunks.concat();
    assert_eq!(sanitize(&chunks), String::from_utf8_lossy(&raw).as_bytes());
}

#[test]
fn every_bare_c1_byte_matches_the_owners_replacement_character() {
    for byte in 0x80..=0x9f_u8 {
        let input = [b'A', byte, b'B', b'C', b'\r', b'\n'];
        for split in 0..=input.len() {
            assert_eq!(
                sanitize(&[&input[..split], &input[split..]]),
                "A\u{fffd}BC\r\n".as_bytes(),
                "byte {byte:#04x}, split {split}"
            );
        }
    }
}

#[test]
fn a_bare_c1_terminator_cannot_end_a_string_the_viewer_keeps_open() {
    // The viewer decodes 0x9c as U+FFFD and keeps buffering the OSC, so
    // treating it as ST would forward the clipboard tail as visible text.
    let input = b"a\x1b]52;c;Zm9vYmFy\x9cdGFpbA==\x07b";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"ab",
            "split {split}"
        );
    }
}

#[test]
fn a_bare_c1_byte_inside_an_allowed_osc_stays_part_of_its_payload() {
    let input = b"a\x1b]2;ti\x9ctle\x07b";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            input.as_slice(),
            "split {split}"
        );
    }
}

#[test]
fn a_bare_c1_byte_inside_an_osc_identifier_fails_closed() {
    // The viewer's OSC identifier would be `2<U+FFFD>` — not a number, so
    // its parser aborts the string. Forwarding it would be a divergence.
    let input = b"a\x1b]2\x9c;title\x07b";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"ab",
            "split {split}"
        );
    }
}

#[test]
fn an_orphaned_c2_lead_byte_cannot_be_recombined_into_a_forged_c1_control() {
    // The input holds no C1 control: `0xc2` is a truncated lead and `0x9f`
    // is invalid UTF-8. Forwarding the lead byte after deleting what sat
    // between them would hand the viewer a real U+009F APC introducer and
    // swallow the rest of the pane's output.
    for input in [
        b"A\xc2\x18\x9fplain text".as_slice(),
        b"A\xc2\x1b]52;c;Zm9v\x07\x9fplain text".as_slice(),
        b"A\xc2\x1b\x1b\x9fplain text".as_slice(),
    ] {
        for split in 0..=input.len() {
            let output = sanitize(&[&input[..split], &input[split..]]);
            assert!(
                !output
                    .windows(2)
                    .any(|pair| pair[0] == 0xc2 && (0x80..=0x9f).contains(&pair[1])),
                "split {split}: forged C1 control in {output:02x?}"
            );
            assert!(
                output.ends_with(b"plain text"),
                "split {split}: pane output must survive, got {output:02x?}"
            );
        }
    }
}

#[test]
fn a_complete_two_byte_character_keeps_both_halves() {
    let input = "A\u{a0}\u{ff}B".as_bytes();
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            input,
            "split {split}"
        );
    }
}

#[test]
fn utf8_encoded_c1_codepoints_do_not_become_terminal_strings() {
    let blocked = b"a\xc2\x9d52;c;Zm9v\xc2\x9cb\xc2\x9fGkitty\xc2\x9cc";
    for split in 0..=blocked.len() {
        assert_eq!(
            sanitize(&[&blocked[..split], &blocked[split..]]),
            b"a52;c;Zm9vbGkittyc",
            "blocked split {split}"
        );
    }

    let allowed = b"a\xc2\x9d2;title\xc2\x9cb";
    for split in 0..=allowed.len() {
        assert_eq!(
            sanitize(&[&allowed[..split], &allowed[split..]]),
            b"a2;titleb",
            "allowed split {split}"
        );
    }
}

#[test]
fn utf8_continuations_that_overlap_c1_controls_are_never_reinterpreted() {
    let input = "a\u{a0}НÜ\u{259c}b".as_bytes();
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            input,
            "split {split}"
        );
    }
}

#[test]
fn utf8_inside_an_allowed_osc_cannot_terminate_it_as_a_c1_string() {
    let input = "\u{1b}]2;Н\u{259c} title\u{1b}\\safe".as_bytes();
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            input,
            "split {split}"
        );
    }
}

#[test]
fn escape_reentry_never_emits_a_forbidden_string_prefix() {
    for input in [
        b"a\x1b\x1b]52;c;WA==\x07b".as_slice(),
        b"a\x1b]2;safe\x1b]52;c;WA==\x07b".as_slice(),
        b"a\x1bPignored\x1b]52;c;WA==\x07\x1b\\b".as_slice(),
    ] {
        for first in 0..=input.len() {
            for second in first..=input.len() {
                assert_eq!(
                    sanitize(&[&input[..first], &input[first..second], &input[second..]]),
                    b"ab",
                    "splits {first}/{second}"
                );
            }
        }
    }
}

#[test]
fn can_and_sub_cancel_every_non_dcs_control_string() {
    for introducer in [
        b"\x1b_payload".as_slice(),
        b"\x1b^payload".as_slice(),
        b"\x1bXpayload".as_slice(),
        b"\x1b]2;payload".as_slice(),
        b"\x1bkpayload".as_slice(),
    ] {
        for cancel in [0x18, 0x1a] {
            let mut input = b"a".to_vec();
            input.extend_from_slice(introducer);
            input.push(cancel);
            input.extend_from_slice(b"VISIBLEb");
            for split in 0..=input.len() {
                assert_eq!(
                    sanitize(&[&input[..split], &input[split..]]),
                    b"aVISIBLEb",
                    "introducer {introducer:02x?}, cancel {cancel:#04x}, split {split}"
                );
            }
        }
    }
}

#[test]
fn dcs_preamble_controls_follow_the_rmux_entry_states() {
    for preamble in [
        b"\x1bP".as_slice(),
        b"\x1bP1".as_slice(),
        b"\x1bP ".as_slice(),
        b"\x1bP:".as_slice(),
    ] {
        for cancel in [0x18, 0x1a] {
            let mut input = b"a".to_vec();
            input.extend_from_slice(preamble);
            input.push(cancel);
            input.extend_from_slice(b"VISIBLEb");
            for split in 0..=input.len() {
                assert_eq!(
                    sanitize(&[&input[..split], &input[split..]]),
                    b"aVISIBLEb",
                    "preamble {preamble:02x?}, cancel {cancel:#04x}, split {split}"
                );
            }
        }
    }

    let input = b"a\x1bP1\x1b[31mVISIBLE\x1b[0mb";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"a\x1b[31mVISIBLE\x1b[0mb",
            "split {split}"
        );
    }
}

#[test]
fn bel_only_terminates_osc_strings() {
    let osc = b"a\x1b]52;c;WA==\x07VISIBLEb";
    for split in 0..=osc.len() {
        assert_eq!(
            sanitize(&[&osc[..split], &osc[split..]]),
            b"aVISIBLEb",
            "OSC split {split}"
        );
    }

    for input in [
        b"a\x1bPqpayload\x07HIDDEN\x1b\\b".as_slice(),
        b"a\x1b_payload\x07HIDDEN\x1b\\b".as_slice(),
        b"a\x1b^payload\x07HIDDEN\x1b\\b".as_slice(),
        b"a\x1bXpayload\x07HIDDEN\x1b\\b".as_slice(),
        b"a\x1bkpayload\x07HIDDEN\x1b\\b".as_slice(),
    ] {
        for split in 0..=input.len() {
            assert_eq!(
                sanitize(&[&input[..split], &input[split..]]),
                b"ab",
                "input {input:02x?}, split {split}"
            );
        }
    }
}

#[test]
fn utf8_c1_inside_an_allowed_osc_fails_closed() {
    let input = b"a\x1b]2;ti\xc2\x9dtle\x07b";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"ab",
            "split {split}"
        );
    }
}

#[test]
fn can_and_sub_cancel_strings_before_following_input_is_reparsed() {
    for cancel in [0x18, 0x1a] {
        let mut input = b"a\x1b]2;safe".to_vec();
        input.push(cancel);
        input.extend_from_slice(b"\x1b]52;c;WA==\x07b");
        for split in 0..=input.len() {
            assert_eq!(
                sanitize(&[&input[..split], &input[split..]]),
                b"ab",
                "cancel {cancel:#x}, split {split}"
            );
        }
    }
}

#[test]
fn dcs_passthrough_never_reparses_payload_controls() {
    for input in [
        b"a\x1bPq\x18VISIBLE\x1b\\b".as_slice(),
        b"a\x1bPq\x1aVISIBLE\x1b\\b".as_slice(),
        b"a\x1bPq\x1b[31mVISIBLE\x1b\\b".as_slice(),
        b"a\x1bPq\x1bAVISIBLE\x1b\\b".as_slice(),
    ] {
        for split in 0..=input.len() {
            assert_eq!(
                sanitize(&[&input[..split], &input[split..]]),
                b"ab",
                "input {input:02x?}, split {split}"
            );
        }
    }
}

#[test]
fn screen_rename_string_is_not_printed_for_the_viewer() {
    let input = b"a\x1bkWINDOWNAME\x1b\\b";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"ab",
            "split {split}"
        );
    }
}

#[test]
fn utf8_c1_codepoints_are_nonprinting_text_not_control_introducers() {
    for codepoint in 0x80..=0x9f_u8 {
        let input = [
            b'a', 0xc2, codepoint, b'V', b'I', b'S', b'I', b'B', b'L', b'E', b'b',
        ];
        for split in 0..=input.len() {
            assert_eq!(
                sanitize(&[&input[..split], &input[split..]]),
                b"aVISIBLEb",
                "C1 {codepoint:#04x}, split {split}"
            );
        }
    }
}

#[test]
fn oversized_osc_discard_ends_at_bell() {
    let mut input = b"before\x1b]2;".to_vec();
    input.resize(input.len() + MAX_BUFFERED_OSC_BYTES + 1, b'x');
    input.extend_from_slice(b"\x07after");
    assert_eq!(sanitize(&[&input]), b"beforeafter");
}

#[test]
fn unknown_osc_is_removed_at_every_fragmentation_boundary() {
    let input = b"before\x1b]999;not-a-web-contract\x07after";
    for split in 0..=input.len() {
        assert_eq!(
            sanitize(&[&input[..split], &input[split..]]),
            b"beforeafter",
            "split {split}"
        );
    }
}

#[test]
fn reset_discards_an_incomplete_control_string() {
    let mut sanitizer = WebTerminalSanitizer::default();
    let mut output = Vec::new();
    sanitizer.push(b"safe\x1b]52;c;partial", &mut output);
    sanitizer.reset();
    sanitizer.push(b"fresh", &mut output);
    assert_eq!(output, b"safefresh");
}
