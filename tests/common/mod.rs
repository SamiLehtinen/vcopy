use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct TestContext {
    pub dir: PathBuf,
    pub src: PathBuf,
    pub dst: PathBuf,
}

impl TestContext {
    pub fn new(name: &str) -> Self {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let base = std::env::temp_dir().join(format!("vcopy_test_{}_{}_{}", name, nanos, id));
        let src = base.join("src");
        let dst = base.join("dst");

        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        Self {
            dir: base,
            src,
            dst,
        }
    }

    pub fn write_src_file(&self, rel: &str, content: &[u8]) {
        let p = self.src.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[allow(dead_code)]
    pub fn read_dst_file(&self, rel: &str) -> Option<Vec<u8>> {
        let p = self.dst.join(rel);
        fs::read(p).ok()
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
