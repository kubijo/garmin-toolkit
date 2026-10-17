use super::{
    Error, File, Limits, MAX_MANIFEST, Manifest, NamedTempFile, Path, Read, Seek, WINDOW_LOG,
    Write, digest, io,
};

pub(super) fn encode(
    output: impl Write,
    manifest: &Manifest,
    database: &Path,
) -> Result<(), Error> {
    let mut encoder = zstd::stream::write::Encoder::new(output, 3)?;
    encoder.include_checksum(true)?;
    encoder.window_log(WINDOW_LOG)?;
    let mut archive = tar::Builder::new(encoder);
    let metadata = serde_json::to_vec(manifest)?;
    append(
        &mut archive,
        "manifest.json",
        metadata.len() as u64,
        metadata.as_slice(),
    )?;
    append(
        &mut archive,
        "storage.sqlite3",
        manifest.database_bytes,
        File::open(database)?,
    )?;
    archive.into_inner()?.finish()?;
    Ok(())
}

fn append<W: Write>(
    archive: &mut tar::Builder<W>,
    name: &str,
    size: u64,
    input: impl Read,
) -> Result<(), Error> {
    // POSIX pax needs no extension records for these fixed names and bounded sizes.
    let mut header = tar::Header::new_ustar();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o600);
    header.set_size(size);
    header.set_cksum();
    archive.append_data(&mut header, name, input)?;
    Ok(())
}

pub(super) fn decode(
    input: impl Read,
    staging: &Path,
    limits: Limits,
) -> Result<(NamedTempFile, Manifest), Error> {
    let mut compressed = NamedTempFile::new_in(staging)?;
    bounded_copy(input, &mut compressed, limits.compressed_bytes)?;
    compressed.rewind()?;
    let mut decoder = zstd::stream::read::Decoder::new(compressed)?.single_frame();
    decoder.window_log_max(WINDOW_LOG)?;
    let mut archive_file = NamedTempFile::new_in(staging)?;
    bounded_copy(&mut decoder, &mut archive_file, limits.archive_bytes())?;
    decoder.finish_frame()?;
    if decoder.finish().read(&mut [0])? != 0 {
        return Err(Error::Invalid(
            "snapshot contains trailing compressed data or multiple frames",
        ));
    }
    archive_file.rewind()?;
    let mut archive = tar::Archive::new(archive_file.as_file_mut());
    // Let tar parse through end markers so appended entries cannot hide behind them.
    archive.set_ignore_zeros(true);
    let mut entries = archive.entries()?.raw(true);
    let mut metadata = entries
        .next()
        .ok_or(Error::Invalid("missing snapshot manifest"))??;
    validate_entry(&metadata, b"manifest.json", MAX_MANIFEST)?;
    let mut bytes = Vec::new();
    metadata.read_to_end(&mut bytes)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    manifest.validate(limits)?;
    drop(metadata);
    let mut entry = entries
        .next()
        .ok_or(Error::Invalid("missing snapshot database"))??;
    validate_entry(&entry, b"storage.sqlite3", limits.database_bytes)?;
    if entry.size() != manifest.database_bytes {
        return Err(Error::Invalid(
            "snapshot database length does not match its manifest",
        ));
    }
    let mut database = NamedTempFile::new_in(staging)?;
    if io::copy(&mut entry, &mut database)? != manifest.database_bytes {
        return Err(Error::Invalid("truncated snapshot database"));
    }
    drop(entry);
    if entries.next().transpose()?.is_some() {
        return Err(Error::Invalid("unexpected snapshot archive entry"));
    }
    if digest(database.as_file_mut())? != manifest.database_sha256 {
        return Err(Error::Invalid(
            "snapshot database digest does not match its manifest",
        ));
    }
    Ok((database, manifest))
}

fn validate_entry<R: Read>(
    entry: &tar::Entry<'_, R>,
    name: &[u8],
    limit: u64,
) -> Result<(), Error> {
    if entry.header().as_ustar().is_none()
        || entry.header().entry_type() != tar::EntryType::Regular
        || entry.header().path_bytes().as_ref() != name
        || entry.header().link_name_bytes().is_some()
        || entry.size() > limit
    {
        return Err(Error::Invalid("invalid snapshot archive entry"));
    }
    Ok(())
}

fn bounded_copy(input: impl Read, output: &mut impl Write, limit: u64) -> Result<(), Error> {
    if io::copy(&mut input.take(limit + 1), output)? > limit {
        return Err(Error::Invalid("snapshot exceeds its size limit"));
    }
    Ok(())
}
