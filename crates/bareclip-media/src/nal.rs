//! Converts between the two ways H.264 and HEVC data is packaged.
//!
//! Encoders here write start codes between units, while MP4 and MKV store each
//! unit with its length in front. Decoder settings also live in a header in those
//! files, so they have to be copied back into the stream after a re-encoded part.

const START_CODE_LONG: [u8; 4] = [0, 0, 0, 1];

/// Whether extra data is in the length-prefixed form used by MP4 and MKV. Data in
/// the other form starts with a start code rather than a version number of 1.
pub fn is_length_prefixed(extradata: &[u8]) -> bool {
    extradata.first() == Some(&1)
}

/// How many bytes each unit's length takes, read from an `avcC` header.
pub fn avc_length_size(extradata: &[u8]) -> Option<usize> {
    extradata.get(4).map(|byte| usize::from(byte & 0b11) + 1)
}

/// How many bytes each unit's length takes, read from an `hvcC` header.
pub fn hevc_length_size(extradata: &[u8]) -> Option<usize> {
    extradata.get(21).map(|byte| usize::from(byte & 0b11) + 1)
}

/// The units in data separated by start codes.
fn split_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= data.len() {
        if data[index..index + 3] == [0, 0, 1] {
            starts.push((index, index + 3));
            index += 3;
        } else {
            index += 1;
        }
    }

    let mut units = Vec::with_capacity(starts.len());
    for (position, &(_, unit_start)) in starts.iter().enumerate() {
        let mut unit_end = starts
            .get(position + 1)
            .map_or(data.len(), |&(next_code, _)| next_code);
        // A four-byte start code has an extra leading zero that is not part of the unit.
        while position + 1 < starts.len() && unit_end > unit_start && data[unit_end - 1] == 0 {
            unit_end -= 1;
        }
        if unit_end > unit_start {
            units.push(&data[unit_start..unit_end]);
        }
    }
    units
}

/// Rewrites start-code data so each unit has its length in front.
pub fn start_codes_to_length_prefixed(data: &[u8], length_size: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len() + 16);
    for unit in split_units(data) {
        push_length_prefixed(&mut output, unit, length_size);
    }
    output
}

fn push_length_prefixed(output: &mut Vec<u8>, unit: &[u8], length_size: usize) {
    let length = (unit.len() as u64).to_be_bytes();
    output.extend_from_slice(&length[8 - length_size..]);
    output.extend_from_slice(unit);
}

fn read_units(
    data: &[u8],
    mut position: usize,
    count: usize,
    units: &mut Vec<Vec<u8>>,
) -> Option<usize> {
    for _ in 0..count {
        let size = usize::from(u16::from_be_bytes([
            *data.get(position)?,
            *data.get(position + 1)?,
        ]));
        position += 2;
        units.push(data.get(position..position + size)?.to_vec());
        position += size;
    }
    Some(position)
}

/// The settings units in an `avcC` header.
pub fn avc_parameter_sets(extradata: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut units = Vec::new();
    let sps_count = usize::from(extradata.get(5)? & 0x1f);
    let position = read_units(extradata, 6, sps_count, &mut units)?;
    let pps_count = usize::from(*extradata.get(position)?);
    read_units(extradata, position + 1, pps_count, &mut units)?;
    Some(units)
}

/// The settings units in an `hvcC` header.
pub fn hevc_parameter_sets(extradata: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut units = Vec::new();
    let arrays = usize::from(*extradata.get(22)?);
    let mut position = 23;
    for _ in 0..arrays {
        let count = usize::from(u16::from_be_bytes([
            *extradata.get(position + 1)?,
            *extradata.get(position + 2)?,
        ]));
        position = read_units(extradata, position + 3, count, &mut units)?;
    }
    Some(units)
}

/// Settings units joined in the same packaging as the stream that will carry them.
pub fn join_units(units: &[Vec<u8>], length_size: Option<usize>) -> Vec<u8> {
    let mut output = Vec::new();
    for unit in units {
        match length_size {
            Some(size) => push_length_prefixed(&mut output, unit, size),
            None => {
                output.extend_from_slice(&START_CODE_LONG);
                output.extend_from_slice(unit);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_byte_start_codes_become_lengths() {
        let data = [0, 0, 0, 1, 0x67, 0xaa, 0, 0, 0, 1, 0x68, 0xbb, 0xcc];

        assert_eq!(
            start_codes_to_length_prefixed(&data, 4),
            [0, 0, 0, 2, 0x67, 0xaa, 0, 0, 0, 3, 0x68, 0xbb, 0xcc]
        );
    }

    #[test]
    fn three_byte_start_codes_become_lengths() {
        let data = [0, 0, 1, 0x65, 0x01, 0, 0, 1, 0x41, 0x02];

        assert_eq!(
            start_codes_to_length_prefixed(&data, 4),
            [0, 0, 0, 2, 0x65, 0x01, 0, 0, 0, 2, 0x41, 0x02]
        );
    }

    #[test]
    fn a_trailing_zero_in_the_last_unit_is_kept() {
        let data = [0, 0, 1, 0x65, 0x01, 0];

        assert_eq!(
            start_codes_to_length_prefixed(&data, 4),
            [0, 0, 0, 3, 0x65, 0x01, 0]
        );
    }

    #[test]
    fn shorter_length_fields_are_respected() {
        let data = [0, 0, 1, 0x65, 0x01];

        assert_eq!(start_codes_to_length_prefixed(&data, 2), [0, 2, 0x65, 0x01]);
    }

    #[test]
    fn data_without_start_codes_produces_nothing() {
        assert!(start_codes_to_length_prefixed(&[1, 2, 3, 4], 4).is_empty());
    }

    fn avcc() -> Vec<u8> {
        vec![
            1, 0x64, 0, 0x28, 0xff, // version, profile and level, length size 4
            0xe1, 0, 2, 0x67, 0xaa, // one SPS of two bytes
            1, 0, 3, 0x68, 0xbb, 0xcc, // one PPS of three bytes
        ]
    }

    #[test]
    fn avc_settings_are_read_from_the_header() {
        assert_eq!(avc_length_size(&avcc()), Some(4));
        assert_eq!(
            avc_parameter_sets(&avcc()),
            Some(vec![vec![0x67, 0xaa], vec![0x68, 0xbb, 0xcc]])
        );
    }

    #[test]
    fn truncated_avc_headers_are_rejected() {
        let mut header = avcc();
        header.truncate(9);

        assert_eq!(avc_parameter_sets(&header), None);
    }

    fn hvcc() -> Vec<u8> {
        let mut header = vec![1];
        header.extend_from_slice(&[0; 20]);
        header.push(0b11); // length size 4
        header.push(2); // two arrays
        header.extend_from_slice(&[0x20, 0, 1, 0, 2, 0x40, 0x01]);
        header.extend_from_slice(&[0x21, 0, 2, 0, 1, 0x42, 0, 1, 0x43]);
        header
    }

    #[test]
    fn hevc_settings_are_read_from_the_header() {
        assert_eq!(hevc_length_size(&hvcc()), Some(4));
        assert_eq!(
            hevc_parameter_sets(&hvcc()),
            Some(vec![vec![0x40, 0x01], vec![0x42], vec![0x43]])
        );
    }

    #[test]
    fn settings_can_be_joined_in_either_packaging() {
        let units = vec![vec![0x67, 0xaa], vec![0x68]];

        assert_eq!(
            join_units(&units, Some(4)),
            [0, 0, 0, 2, 0x67, 0xaa, 0, 0, 0, 1, 0x68]
        );
        assert_eq!(
            join_units(&units, None),
            [0, 0, 0, 1, 0x67, 0xaa, 0, 0, 0, 1, 0x68]
        );
    }

    #[test]
    fn the_packaging_is_detected_from_the_first_byte() {
        assert!(is_length_prefixed(&avcc()));
        assert!(!is_length_prefixed(&[0, 0, 0, 1, 0x67]));
        assert!(!is_length_prefixed(&[]));
    }
}
