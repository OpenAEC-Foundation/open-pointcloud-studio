//! Buffered file reader whose seeks keep the buffered window.
//!
//! Page-oriented decoders seek before every small page. `std::io::BufReader`
//! drops its buffer on each seek, so every page becomes a separate request to
//! the operating system, which is very slow on a network share. This reader
//! answers seeks with arithmetic and refills a large window only when a read
//! leaves it.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const FIRST_WINDOW: usize = 64 * 1024;
const LARGEST_WINDOW: usize = 4 * 1024 * 1024;

pub(crate) struct WindowReader {
    file: File,
    length: u64,
    position: u64,
    window_start: u64,
    window: Vec<u8>,
    next_window: usize,
    #[cfg(test)]
    refills: usize,
}

impl WindowReader {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Ok(Self {
            file,
            length,
            position: 0,
            window_start: 0,
            window: Vec::new(),
            next_window: FIRST_WINDOW,
            #[cfg(test)]
            refills: 0,
        })
    }

    fn window_end(&self) -> u64 {
        self.window_start + self.window.len() as u64
    }

    fn refill(&mut self) -> io::Result<()> {
        // Sequential access grows the window; a jump starts small again so a
        // metadata lookup does not pull megabytes it will never use.
        let sequential = !self.window.is_empty() && self.position == self.window_end();
        self.next_window = if sequential {
            (self.next_window * 2).min(LARGEST_WINDOW)
        } else {
            FIRST_WINDOW
        };
        let wanted = (self.length.saturating_sub(self.position)).min(self.next_window as u64);
        self.window.resize(wanted as usize, 0);
        self.window_start = self.position;
        self.file.seek(SeekFrom::Start(self.position))?;
        let mut filled = 0;
        while filled < self.window.len() {
            match self.file.read(&mut self.window[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.window.clear();
                    return Err(error);
                }
            }
        }
        self.window.truncate(filled);
        #[cfg(test)]
        {
            self.refills += 1;
        }
        Ok(())
    }
}

impl Read for WindowReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position >= self.length {
            return Ok(0);
        }
        if self.position < self.window_start || self.position >= self.window_end() {
            self.refill()?;
            if self.window.is_empty() {
                return Ok(0);
            }
        }
        let offset = (self.position - self.window_start) as usize;
        let count = buffer.len().min(self.window.len() - offset);
        buffer[..count].copy_from_slice(&self.window[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for WindowReader {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let position = match target {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => self.length.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        };
        self.position = position.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "seek before start of file")
        })?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn patterned_file(length: usize) -> (tempfile::NamedTempFile, Vec<u8>) {
        let bytes: Vec<u8> = (0..length)
            .map(|index| (index.wrapping_mul(31) ^ (index >> 9)) as u8)
            .collect();
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        file.flush().unwrap();
        (file, bytes)
    }

    #[test]
    fn page_wise_seeks_are_served_from_few_large_reads() {
        let (file, bytes) = patterned_file(3 * 1024 * 1024 + 517);
        let mut reader = WindowReader::open(file.path()).unwrap();
        assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), bytes.len() as u64);
        let mut page = [0u8; 1024];
        for start in (0..bytes.len() - 1024).step_by(1024) {
            reader.seek(SeekFrom::Start(start as u64)).unwrap();
            reader.read_exact(&mut page).unwrap();
            assert_eq!(page[..], bytes[start..start + 1024]);
        }
        // 3 MiB in 1 KiB pages is about 3,000 seeks; the window grows from
        // 64 KiB by doubling, so only a handful of file reads are needed.
        assert!(reader.refills <= 8, "{} refills", reader.refills);
    }

    #[test]
    fn jumps_and_the_file_end_return_exact_bytes() {
        let (file, bytes) = patterned_file(300_000);
        let mut reader = WindowReader::open(file.path()).unwrap();
        let mut head = [0u8; 48];
        reader.read_exact(&mut head).unwrap();
        assert_eq!(head[..], bytes[..48]);

        reader.seek(SeekFrom::End(-100)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail[..], bytes[bytes.len() - 100..]);
        assert_eq!(reader.read(&mut head).unwrap(), 0);

        reader.seek(SeekFrom::Start(150_000)).unwrap();
        reader.seek(SeekFrom::Current(-10)).unwrap();
        let mut middle = [0u8; 20];
        reader.read_exact(&mut middle).unwrap();
        assert_eq!(middle[..], bytes[149_990..150_010]);

        assert!(reader.seek(SeekFrom::Current(-1_000_000)).is_err());
        reader
            .seek(SeekFrom::Start(bytes.len() as u64 + 5))
            .unwrap();
        assert_eq!(reader.read(&mut head).unwrap(), 0);
    }
}
