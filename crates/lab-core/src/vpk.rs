//! Minimal VPK v2 writer for a single root-level file.
//!
//! CS2 resolves files found in VPKs before loose files, so the custom `botprofile.db` has to
//! ship in its own VPK mounted ahead of `csgo` in gameinfo.gi. The file data is stored in the
//! directory file itself (archive index 0x7fff), and the MD5 section is written the way
//! Valve's tool writes it (tree, archive-MD5 section, whole file).

use md5::{Digest, Md5};

const SIGNATURE: u32 = 0x55aa_1234;
const VERSION: u32 = 2;
const HEADER_LEN: usize = 28;
/// Archive index meaning "data lives in this _dir.vpk, after the tree".
const DIR_ARCHIVE: u16 = 0x7fff;
const ENTRY_TERMINATOR: u16 = 0xffff;
/// Tree checksum + archive-MD5-section checksum + whole-file checksum.
const OTHER_MD5_LEN: u32 = 48;

/// Builds a VPK holding `data` at the root as `file_name` (e.g. `botprofile.db`).
pub fn single_file(file_name: &str, data: &[u8]) -> Vec<u8> {
    let (stem, ext) = file_name
        .rsplit_once('.')
        .expect("file name must have an extension");

    let mut tree = Vec::new();
    for s in [ext, " ", stem] {
        // " " is the VPK spelling of the root directory.
        tree.extend_from_slice(s.as_bytes());
        tree.push(0);
    }
    tree.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
    tree.extend_from_slice(&0u16.to_le_bytes()); // preload bytes
    tree.extend_from_slice(&DIR_ARCHIVE.to_le_bytes());
    tree.extend_from_slice(&0u32.to_le_bytes()); // offset into the data section
    tree.extend_from_slice(&(data.len() as u32).to_le_bytes());
    tree.extend_from_slice(&ENTRY_TERMINATOR.to_le_bytes());
    tree.extend_from_slice(&[0, 0, 0]); // end of names, paths, extensions

    let mut out = Vec::with_capacity(HEADER_LEN + tree.len() + data.len() + 48);
    for v in [
        SIGNATURE,
        VERSION,
        tree.len() as u32,
        data.len() as u32,
        0, // archive MD5 section size
        OTHER_MD5_LEN,
        0, // signature section size
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&tree);
    out.extend_from_slice(data);
    // The archive MD5 section is empty; its checksum is the MD5 of nothing.
    out.extend_from_slice(&Md5::digest(&tree));
    out.extend_from_slice(&Md5::digest([]));
    let whole = Md5::digest(&out);
    out.extend_from_slice(&whole);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }

    #[test]
    fn round_trips_one_file() {
        let data = b"Default\r\nEnd\r\n";
        let v = single_file("botprofile.db", data);
        assert_eq!(u32_at(&v, 0), SIGNATURE);
        assert_eq!(u32_at(&v, 4), VERSION);
        let tree_len = u32_at(&v, 8) as usize;
        assert_eq!(u32_at(&v, 12) as usize, data.len());

        let tree = &v[HEADER_LEN..HEADER_LEN + tree_len];
        assert!(tree.starts_with(b"db\0 \0botprofile\0"));
        let e = b"db\0 \0botprofile\0".len();
        assert_eq!(u32_at(tree, e), crc32fast::hash(data));
        assert_eq!(&tree[e + 6..e + 8], &DIR_ARCHIVE.to_le_bytes());
        assert_eq!(u32_at(tree, e + 8), 0);
        assert_eq!(u32_at(tree, e + 12) as usize, data.len());
        assert!(tree.ends_with(&[0xff, 0xff, 0, 0, 0]));

        let data_start = HEADER_LEN + tree_len;
        assert_eq!(&v[data_start..data_start + data.len()], data);

        let md5 = &v[data_start + data.len()..];
        assert_eq!(md5.len(), 48);
        assert_eq!(&md5[..16], Md5::digest(tree).as_slice());
        assert_eq!(&md5[32..], Md5::digest(&v[..v.len() - 16]).as_slice());
    }
}
