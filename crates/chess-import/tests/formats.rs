//! Every way into the importer: plain PGN, zip archives (with PGN in a legacy code
//! page and in folders), an unpacked ChessBase database on disk.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chess_import::{Format, ImportError, Opened};

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!(
            "chess-import-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&p).expect("temp dir");
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(path: &Path) -> Result<Opened, ImportError> {
    let never = AtomicBool::new(false);
    chess_import::open(path, &mut |_| {}, &never)
}

const GAME: &str = "[Event \"Test\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7# 1-0\n";

#[test]
fn plain_pgn_is_unchanged() {
    let dir = TempDir::new("pgn");
    let p = dir.0.join("game.pgn");
    std::fs::write(&p, GAME).expect("write");
    let o = open(&p).expect("opens");
    assert_eq!(o.format, Format::Pgn);
    assert_eq!(o.db.len(), 1);
    assert!(o.notes.is_empty());
}

#[test]
fn pgn_and_archives_open_from_bytes() {
    let never = AtomicBool::new(false);
    let pgn = chess_import::open_bytes("game.pgn", GAME.as_bytes().to_vec(), &mut |_| {}, &never)
        .expect("opens");
    assert_eq!(pgn.format, Format::Pgn);
    assert_eq!(pgn.db.len(), 1);
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("one.pgn", zip::write::SimpleFileOptions::default())
        .expect("entry");
    zip.write_all(GAME.as_bytes()).expect("data");
    let bytes = zip.finish().expect("zip").into_inner();
    let z = chess_import::open_bytes("games.zip", bytes, &mut |_| {}, &never).expect("opens");
    assert_eq!(z.format, Format::Zip);
    assert_eq!(z.db.raw_text(0), pgn.db.raw_text(0));
    // A .cbh alone is not a database: its other files are needed.
    assert!(matches!(
        chess_import::open_bytes("base.cbh", vec![0; 46], &mut |_| {}, &never),
        Err(ImportError::Unsupported(
            chess_import::Unsupported::ChessBaseFiles
        ))
    ));
}

#[test]
fn zip_with_legacy_pgn_and_folders() {
    let dir = TempDir::new("zip");
    let p = dir.0.join("games.zip");
    let file = std::fs::File::create(&p).expect("create");
    let mut zip = zip::ZipWriter::new(file);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("one.pgn", stored).expect("entry");
    zip.write_all(GAME.as_bytes()).expect("data");
    // A Russian comment in Windows-1251, inside a folder.
    let (cp1251, _, _) = encoding_rs::WINDOWS_1251.encode(
        "[Event \"Тест\"]\n[White \"Алехин\"]\n[Black \"Капабланка\"]\n[Result \"*\"]\n\n1. d4 {Хороший ход} d5 *\n",
    );
    zip.add_directory("sub/", stored).expect("dir");
    zip.start_file("sub/two.PGN", stored).expect("entry");
    zip.write_all(&cp1251).expect("data");
    zip.start_file("readme.txt", stored).expect("entry");
    zip.write_all(b"not a database").expect("data");
    zip.finish().expect("finish");

    let o = open(&p).expect("opens");
    assert_eq!(o.format, Format::Zip);
    assert_eq!(o.db.len(), 2);
    let second = o.db.raw_text(1).expect("text");
    assert!(
        second.contains("Алехин") && second.contains("Хороший ход"),
        "{second}"
    );

    // The same archive under another extension is still recognized by its content.
    let renamed = dir.0.join("games.bin");
    std::fs::copy(&p, &renamed).expect("copy");
    assert_eq!(open(&renamed).expect("opens").format, Format::Zip);
}

#[test]
fn zip_without_databases_and_unsupported_files() {
    let dir = TempDir::new("empty");
    let p = dir.0.join("empty.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&p).expect("create"));
    zip.start_file("notes.txt", zip::write::SimpleFileOptions::default())
        .expect("entry");
    zip.write_all(b"hello").expect("data");
    zip.finish().expect("finish");
    assert!(matches!(open(&p), Err(ImportError::Empty)));

    let cbz = dir.0.join("locked.cbz");
    std::fs::write(&cbz, b"whatever").expect("write");
    assert!(matches!(
        open(&cbz),
        Err(ImportError::Unsupported(
            chess_import::Unsupported::Encrypted
        ))
    ));
    let broken = dir.0.join("broken.cbv");
    std::fs::write(&broken, [8u8, 0, 1, 0, 0x95, 0, 0, 0, 1, 2, 3]).expect("write");
    assert!(matches!(open(&broken), Err(ImportError::Corrupt(_))));
}

#[test]
fn unpacked_chessbase_database_matches_its_archive() {
    // Unpack a book archive from the samples to disk and open its .cbh directly.
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pgnFilesforExample");
    let Some(archive) = std::fs::read_dir(&samples).ok().and_then(|d| {
        d.flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "7z"))
    }) else {
        eprintln!("skipped: no .7z sample");
        return;
    };
    let dir = TempDir::new("cbh");
    let mut cbh = None;
    let mut reader = sevenz_rust2::ArchiveReader::new(
        std::fs::File::open(&archive).expect("open"),
        sevenz_rust2::Password::empty(),
    )
    .expect("7z");
    reader
        .for_each_entries(|entry, r| {
            let mut data = Vec::new();
            r.read_to_end(&mut data)?;
            let name = entry
                .name()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_owned();
            if !entry.is_directory() && !name.is_empty() && !name.ends_with(".cbv") {
                let p = dir.0.join(&name);
                std::fs::write(&p, data)?;
                if name.ends_with(".cbh") {
                    cbh = Some(p);
                }
            }
            Ok(true)
        })
        .expect("unpack");
    let cbh = cbh.expect("archive holds a .cbh");
    let from_disk = open(&cbh).expect("opens");
    assert_eq!(from_disk.format, Format::ChessBase);
    let from_archive = open(&archive).expect("opens");
    assert_eq!(from_disk.db.len(), from_archive.db.len());
    // The same without the file system (Android): the archive as bytes, the database
    // as the contents of its files.
    let never = AtomicBool::new(false);
    let name = archive.file_name().unwrap().to_string_lossy().into_owned();
    let bytes = std::fs::read(&archive).expect("read");
    let archive_bytes = chess_import::open_bytes(&name, bytes, &mut |_| {}, &never).expect("opens");
    let files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dir.0)
        .expect("dir")
        .flatten()
        .map(|e| {
            let p = e.path();
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).expect("read"),
            )
        })
        .collect();
    let cbh_name = cbh.file_name().unwrap().to_string_lossy().into_owned();
    let from_files =
        chess_import::open_chessbase_files(&cbh_name, &files, &mut |_| {}, &never).expect("opens");
    assert_eq!(from_files.format, Format::ChessBase);
    assert_eq!(from_files.db.len(), from_disk.db.len());
    assert_eq!(archive_bytes.db.len(), from_archive.db.len());
    for i in 0..from_disk.db.len() {
        assert_eq!(
            from_files.db.raw_text(i),
            from_disk.db.raw_text(i),
            "record {i}"
        );
        assert_eq!(
            archive_bytes.db.raw_text(i),
            from_archive.db.raw_text(i),
            "record {i}"
        );
    }
    for i in 0..from_disk.db.len() {
        assert_eq!(
            from_disk.db.raw_text(i),
            from_archive.db.raw_text(i),
            "record {i}"
        );
    }
}
