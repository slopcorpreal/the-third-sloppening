#![allow(dead_code)]

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

use memmap2::{Mmap, MmapOptions};

pub enum Backing {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

pub struct MmapBuffer {
    path: Option<PathBuf>,
    backing: Backing,
}

impl MmapBuffer {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mapped = unsafe { MmapOptions::new().map(&file)? };

        Ok(Self {
            path: Some(path.to_path_buf()),
            backing: Backing::Mapped(mapped),
        })
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            path: None,
            backing: Backing::Owned(bytes),
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        match &self.backing {
            Backing::Mapped(m) => m,
            Backing::Owned(v) => v,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
