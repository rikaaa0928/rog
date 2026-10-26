#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(unix)]
fn raise_nofile_limit() {
    unsafe {
        let mut rlim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) == 0 {
            let target = rlim.rlim_max.min(524_288);
            if rlim.rlim_cur < target {
                rlim.rlim_cur = target;
                let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &rlim);
            }
        }
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    #[cfg(unix)]
    raise_nofile_limit();
    rog::run_from_env().await
}
