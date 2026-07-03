use sdr_aaronia_rs::file_source::RtsaSource;
use std::io::Write;
use tempfile::NamedTempFile;

// Helper to write bytes to a temp file and return its path
fn create_temp_file(data: &[u8]) -> std::path::PathBuf {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(data).unwrap();
    file.into_temp_path().keep().unwrap()
}

#[test]
fn test_rtsa_invalid_signature() {
    // A file of pure garbage bytes must fail gracefully — no panic, no
    // hang, just an `Err`. The current parser scans for the trailing
    // DSFT chunk and errors when it can't find one, which is the
    // observed behaviour we pin here.
    let data = b"NOT_A_VALID_RTSA_FILE_JUST_SOME_GARBAGE_BYTES";
    let path = create_temp_file(data);

    let result = RtsaSource::open(&path);

    match result {
        Err(e) => {
            let err_string = e.to_string();
            // Any of these indicate a structured rejection rather than
            // a panic / hang. Stay loose because the exact wording is
            // an implementation detail.
            assert!(
                err_string.contains("DSFT")
                    || err_string.contains("chunk")
                    || err_string.contains("magic")
                    || err_string.contains("signature"),
                "expected structured rejection error, got: {err_string}"
            );
        }
        Ok(_) => panic!("Expected RtsaSource::open to fail on garbage bytes"),
    }

    std::fs::remove_file(path).unwrap();
}

#[test]
fn test_rtsa_truncated_dsfh_header() {
    // The magic bytes for DSFH header is "DSFH" + length (4 bytes) + version (4 bytes) + ...
    let data = b"DSFH\x10\x00\x00\x00\x01\x00\x00\x00"; // Truncated right after version
    let path = create_temp_file(data);

    let result = RtsaSource::open(&path);

    match result {
        Err(e) => {
            let err_string = e.to_string();
            assert!(
                err_string.contains("DSFT")
                    || err_string.contains("chunk")
                    || err_string.contains("EOF")
                    || err_string.contains("read")
                    || err_string.contains("parse"),
                "expected structured rejection error, got: {err_string}"
            );
        }
        Ok(_) => panic!("Expected RtsaSource::open to fail with truncated header"),
    }

    std::fs::remove_file(path).unwrap();
}

#[test]
fn test_rtsa_invalid_chunk_type() {
    // Valid DSFH header but then an invalid chunk type
    let mut data = Vec::new();
    // DSFH
    data.extend_from_slice(b"DSFH");
    // Size (large enough to cover DSFH struct)
    data.extend_from_slice(&[40, 0, 0, 0]);
    // Fill the rest of DSFH with zeros
    data.extend_from_slice(&[0; 40]);

    // Invalid chunk "BADD"
    data.extend_from_slice(b"BADD");
    // Size
    data.extend_from_slice(&[10, 0, 0, 0]);
    // Garbage
    data.extend_from_slice(&[0; 10]);

    let path = create_temp_file(&data);

    let result = RtsaSource::open(&path);

    match result {
        Err(e) => {
            let err_string = e.to_string();
            // Should complain about invalid chunk or unknown chunk
            assert!(err_string.contains("chunk") || err_string.contains("parse"));
        }
        Ok(_) => {
            // It might succeed if it just skips unknown chunks, which could be valid.
            // But if it succeeds, it must not panic.
        }
    }

    std::fs::remove_file(path).unwrap();
}
