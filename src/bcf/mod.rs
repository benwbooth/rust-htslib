
use std::ffi;
use std::convert::AsRef;
use std::path::Path;

pub mod record;
pub mod header;

use htslib;
use bcf::header::{HeaderView, SampleSubset};

pub use bcf::header::Header;
pub use bcf::record::Record;


pub struct Reader {
    inner: *mut htslib::vcf::htsFile,
    pub header: HeaderView,
}


unsafe impl Send for Reader {}


impl Reader {
   pub fn new<P: AsRef<Path>>(path: &P) -> Self {
        let htsfile = bcf_open(path, b"r");
        let header = unsafe { htslib::vcf::bcf_hdr_read(htsfile) };
        Reader { inner: htsfile, header: HeaderView::new(header) }
    }

    pub fn read(&self, record: &mut record::Record) -> Result<(), ReadError> {
        match unsafe { htslib::vcf::bcf_read(self.inner, self.header.inner, record.inner) } {
            0  => {
                record.header = self.header.inner;
                Ok(())
            },
            -1 => Err(ReadError::NoMoreRecord),
            _  => Err(ReadError::Invalid),
        }
    }

    pub fn records(&self) -> Records {
        Records { reader: self }
    }
}


impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            htslib::vcf::bcf_hdr_destroy(self.header.inner);
            htslib::vcf::hts_close(self.inner);
        }
    }
}


pub struct Writer {
    inner: *mut htslib::vcf::htsFile,
    pub header: HeaderView,
    subset: Option<SampleSubset>,
}


unsafe impl Send for Writer {}


impl Writer {
    pub fn new<P: AsRef<Path>>(path: &P, header: &Header, uncompressed: bool, vcf: bool) -> Self {
        let mode: &[u8] = match (uncompressed, vcf) {
            (true, true)   => b"w",
            (false, true)  => b"wz",
            (true, false)  => b"wu",
            (false, false) => b"wb",
        };

        let htsfile = bcf_open(path, mode);
        unsafe { htslib::vcf::bcf_hdr_write(htsfile, header.inner) };
        Writer {
            inner: htsfile,
            header: HeaderView::new(unsafe { htslib::vcf::bcf_hdr_dup(header.inner) }),
            subset: header.subset.clone()
        }
    }

    /// Translate record to header of this writer.
    pub fn translate(&mut self, record: &mut record::Record) {
        unsafe {
            htslib::vcf::bcf_translate(self.header.inner, record.header, record.inner);
        }
        record.header = self.header.inner;
    }

    /// Subset samples of record to match header of this writer.
    pub fn subset(&mut self, record: &mut record::Record) {
        match self.subset {
            Some(ref mut subset) => unsafe {
                htslib::vcf::bcf_subset(self.header.inner, record.inner, subset.len() as i32, subset.as_mut_ptr());
            },
            None         => ()
        }
    }

    pub fn write(&mut self, record: &record::Record) -> Result<(), ()> {
        if unsafe { htslib::vcf::bcf_write(self.inner, self.header.inner, record.inner) } == -1 {
            Err(())
        }
        else {
            Ok(())
        }
    }
}


impl Drop for Writer {
    fn drop(&mut self) {
        unsafe {
            htslib::vcf::bcf_hdr_destroy(self.header.inner);
            htslib::vcf::hts_close(self.inner);
        }
    }
}


pub struct Records<'a> {
    reader: &'a Reader,
}


impl<'a> Iterator for Records<'a> {
    type Item = Result<record::Record, ReadError>;

    fn next(&mut self) -> Option<Result<record::Record, ReadError>> {
        let mut record = record::Record::new();
        match self.reader.read(&mut record) {
            Err(ReadError::NoMoreRecord) => None,
            Err(e)                       => Some(Err(e)),
            Ok(())                       => Some(Ok(record)),
        }
    }
}


/// Wrapper for opening a BCF file.
fn bcf_open<P: AsRef<Path>>(path: &P, mode: &[u8]) -> *mut htslib::vcf::htsFile {
    unsafe {
        htslib::vcf::hts_open(
            path.as_ref().as_os_str().to_cstring().unwrap().as_ptr(),
            ffi::CString::new(mode).unwrap().as_ptr()
        )
    }
}


pub enum ReadError {
    Invalid,
    NoMoreRecord,
}


#[cfg(test)]
mod tests {
    extern crate tempdir;
    use super::*;
    use std::path::Path;

    fn _test_read<P: AsRef<Path>>(path: &P) {
        let bcf = Reader::new(path);
        assert_eq!(bcf.header.samples(), [b"NA12878.subsample-0.25-0"]);

        for (i, rec) in bcf.records().enumerate() {
            let mut record = rec.ok().expect("Error reading record.");
            assert_eq!(record.sample_count(), 1);

            assert_eq!(record.rid().expect("Error reading rid."), 0);
            assert_eq!(record.pos(), 10021 + i as u32);
            assert_eq!(record.qual(), 0f32);
            assert_eq!(record.info(b"MQ0F").float().ok().expect("Error reading info."), [1.0]);
            if i == 59 {
                assert_eq!(record.info(b"SGB").float().ok().expect("Error reading info."), [-0.379885]);
            }
            // the artificial "not observed" allele is present in each record.
            assert_eq!(record.alleles().iter().last().unwrap(), b"<X>");

            let mut fmt = record.format(b"PL");
            let pl = fmt.integer().ok().expect("Error reading format.");
            assert_eq!(pl.len(), 1);
            if i == 59 {
                assert_eq!(pl[0].len(), 6);
            }
            else {
                assert_eq!(pl[0].len(), 3);
            }
        }
    }

    #[test]
    fn test_read() {
        _test_read(&"test.bcf");
    }

    #[test]
    fn test_write() {
        let bcf = Reader::new(&"test_multi.bcf");
        let tmp = tempdir::TempDir::new("rust-htslib").ok().expect("Cannot create temp dir");
        let bcfpath = tmp.path().join("test.bcf");
        println!("{:?}", bcfpath);
        {
            let header = Header::subset_template(&bcf.header, &[b"NA12878.subsample-0.25-0"]).ok().expect("Error subsetting samples.");
            let mut writer = Writer::new(&bcfpath, &header, false, false);
            for rec in bcf.records() {
                let mut record = rec.ok().expect("Error reading record.");
                writer.translate(&mut record);
                writer.subset(&mut record);
                record.trim_alleles().ok().expect("Error trimming alleles.");
                writer.write(&record).ok().expect("Error writing record");
            }
        }
        {
            _test_read(&bcfpath);
        }
        tmp.close().ok().expect("Failed to delete temp dir");
    }
}
