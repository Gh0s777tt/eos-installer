use std::{
    cmp,
    convert::TryInto,
    fs::{File, OpenOptions},
    io::{Read, Result, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Debug)]
pub struct DiskWrapper {
    disk: File,
    size: u64,
    block: Box<[u8]>,
    seek: u64,
}

enum Buffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

/// Logical block size of the target — asked of the device, not assumed.
///
/// WHY THIS EXISTS. This was `let block_size = 512;` with a TODO. `installer.rs` then does
///
///     let gpt_block_size = match block_size {
///         512 => gpt::disk::LogicalBlockSize::Lb512,
///         _ => bail!("block size {block_size} not supported"),
///     };
///
/// Because the value was a constant, that `_` arm was unreachable: the guard could never
/// fire. On a 4Kn drive the installer therefore did NOT refuse — it laid out GPT on the
/// wrong sector size and produced a partition table the firmware cannot read. A check that
/// can only pass is not a check; this is what makes that one able to fail.
///
/// WHERE EACH ANSWER COMES FROM:
///
/// * Redox — `st_blksize`, which the block driver fills in from the device itself
///   (`drivers/storage/driver-block/src/lib.rs:538`: `stat.st_blksize = disk.block_size()`).
///   This is the path that matters for a bare-metal install, because that install runs
///   on Redox.
/// * Linux, block device — `BLKSSZGET`. `st_blksize` is a preferred-I/O hint there, not the
///   logical sector size GPT counts LBAs in, so it is the wrong number to trust.
/// * Anything else, including every image FILE — 512. An image has no sector size of its
///   own; the GPT written inside it is in 512-byte LBAs and the image is copied onto a
///   device afterwards. This is also why the original TODO was right that `blksize()`
///   "works on disks but not image files".
#[cfg(target_os = "redox")]
fn logical_block_size(_disk: &File, metadata: &std::fs::Metadata) -> Result<usize> {
    use std::os::unix::fs::MetadataExt;
    let blksize = metadata.blksize();
    // A scheme that reports 0 has told us nothing; assuming 512 there would put us back
    // where we started, so say so instead.
    if blksize == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "device reported a logical block size of 0",
        ));
    }
    Ok(blksize as usize)
}

#[cfg(target_os = "linux")]
fn logical_block_size(disk: &File, metadata: &std::fs::Metadata) -> Result<usize> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileTypeExt;

    if !metadata.file_type().is_block_device() {
        return Ok(512);
    }

    // BLKSSZGET == _IO(0x12, 104): the LOGICAL sector size, which is the unit GPT LBAs are
    // counted in. Deliberately not BLKPBSZGET (physical): a 512e drive reports 4096 physical
    // and 512 logical, and GPT follows the logical one.
    const BLKSSZGET: libc::c_ulong = 0x1268;
    let mut sector_size: libc::c_int = 0;
    let rc = unsafe { libc::ioctl(disk.as_raw_fd(), BLKSSZGET, &mut sector_size) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if sector_size <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("device reported a logical block size of {sector_size}"),
        ));
    }
    Ok(sector_size as usize)
}

#[cfg(not(any(target_os = "redox", target_os = "linux")))]
fn logical_block_size(_disk: &File, _metadata: &std::fs::Metadata) -> Result<usize> {
    // macOS would need DKIOCGETBLOCKSIZE. Nothing installs to a raw device from macOS in
    // this project -- the build runs in a Linux container and the install runs on Redox --
    // so this is left as the image-file answer rather than guessed at.
    Ok(512)
}

impl DiskWrapper {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let disk = OpenOptions::new().read(true).write(true).open(path)?;
        let metadata = disk.metadata()?;
        let size = metadata.len();
        let block_size = logical_block_size(&disk, &metadata)?;
        let block = vec![0u8; block_size].into_boxed_slice();
        Ok(Self {
            disk,
            size,
            block,
            seek: 0,
        })
    }

    pub fn block_size(&self) -> usize {
        self.block.len()
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn io<'a>(&mut self, buf: &mut Buffer<'a>) -> Result<usize> {
        let buf_len = match buf {
            Buffer::Read(read) => read.len(),
            Buffer::Write(write) => write.len(),
        };
        let block_len: u64 = self.block.len().try_into().unwrap();

        // Do aligned I/O quickly
        if self.seek % block_len == 0 && buf_len as u64 % block_len == 0 {
            self.disk.seek(SeekFrom::Start(self.seek))?;
            match buf {
                Buffer::Read(read) => self.disk.read_exact(read)?,
                Buffer::Write(write) => self.disk.write_all(write)?,
            }
            self.seek = self.seek.checked_add(buf_len.try_into().unwrap()).unwrap();
            return Ok(buf_len);
        }

        let mut i = 0;
        while i < buf_len {
            let block = self.seek / block_len;
            let offset: usize = (self.seek % block_len).try_into().unwrap();
            let remaining = buf_len.checked_sub(i).unwrap();
            let len = cmp::min(remaining, self.block.len().checked_sub(offset).unwrap());

            self.disk
                .seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
            self.disk.read_exact(&mut self.block)?;

            match buf {
                Buffer::Read(read) => {
                    read[i..i.checked_add(len).unwrap()]
                        .copy_from_slice(&self.block[offset..offset.checked_add(len).unwrap()]);
                }
                Buffer::Write(write) => {
                    self.block[offset..offset.checked_add(len).unwrap()]
                        .copy_from_slice(&write[i..i.checked_add(len).unwrap()]);

                    self.disk
                        .seek(SeekFrom::Start(block.checked_mul(block_len).unwrap()))?;
                    self.disk.write_all(&mut self.block)?;
                }
            }

            i = i.checked_add(len).unwrap();
            self.seek = self.seek.checked_add(len.try_into().unwrap()).unwrap();
        }

        Ok(i)
    }
}

impl Read for DiskWrapper {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.io(&mut Buffer::Read(buf))
    }
}

impl Seek for DiskWrapper {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let current: i64 = self.seek.try_into().unwrap();
        let end: i64 = self.size.try_into().unwrap();
        self.seek = match pos {
            SeekFrom::Start(offset) => cmp::min(self.size, offset),
            SeekFrom::End(offset) => cmp::max(0, cmp::min(end, end.wrapping_add(offset))) as u64,
            SeekFrom::Current(offset) => {
                cmp::max(0, cmp::min(end, current.wrapping_add(offset))) as u64
            }
        };
        Ok(self.seek)
    }
}

impl Write for DiskWrapper {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.io(&mut Buffer::Write(buf))
    }

    fn flush(&mut self) -> Result<()> {
        self.disk.flush()
    }
}
