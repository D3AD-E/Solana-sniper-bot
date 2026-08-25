//! Writing to several sockets with one syscall.
//!
//! A provider with eight regional endpoints costs eight `write` syscalls, one after another,
//! and the last region goes out ~11µs after the first. `io_uring` submits all of them in a
//! single `io_uring_enter`, so they leave together.
//!
//! Only plain TCP goes through here. TLS endpoints keep the ordinary path, because rustls
//! owns its own record buffering and there is nothing to batch.
//!
//! Everything degrades safely: if the ring cannot be created — old kernel, container policy,
//! not Linux — `BatchWriter::new` returns `None` and the caller writes sequentially.

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::RawFd;

    use io_uring::{opcode, types, IoUring};

    pub struct BatchWriter {
        ring: IoUring,
        capacity: usize,
    }

    impl BatchWriter {
        pub fn new(capacity: usize) -> Option<Self> {
            // one slot per endpoint, rounded up to the power of two the ring wants
            let entries = capacity.next_power_of_two().max(8) as u32;
            let ring = IoUring::new(entries).ok()?;
            Some(Self { ring, capacity })
        }

        pub fn capacity(&self) -> usize {
            self.capacity
        }

        /// Submits one write per item and waits for all of them.
        ///
        /// Returns the number of bytes written for each item, in the order given. A short
        /// write is reported as-is so the caller can finish it the ordinary way; an error is
        /// reported as `Err`.
        ///
        /// # Safety of the buffers
        ///
        /// The kernel reads from `items` while the call is in flight, and the call does not
        /// return until every completion has been reaped, so the borrow outlives the
        /// kernel's use of it.
        pub fn write_all(&mut self, items: &[(RawFd, &[u8])]) -> Vec<std::io::Result<usize>> {
            let mut results: Vec<std::io::Result<usize>> =
                (0..items.len()).map(|_| Ok(0)).collect();
            if items.is_empty() {
                return results;
            }

            let mut submitted = 0usize;
            for (i, (fd, buf)) in items.iter().enumerate() {
                let entry = opcode::Write::new(types::Fd(*fd), buf.as_ptr(), buf.len() as u32)
                    .build()
                    .user_data(i as u64);
                // safety: `buf` outlives this function, which waits for every completion
                if unsafe { self.ring.submission().push(&entry) }.is_err() {
                    break;
                }
                submitted += 1;
            }

            if submitted == 0 {
                return results;
            }
            if self.ring.submit_and_wait(submitted).is_err() {
                for r in results.iter_mut() {
                    *r = Err(std::io::Error::other("io_uring submit failed"));
                }
                return results;
            }

            let mut reaped = 0usize;
            for cqe in self.ring.completion() {
                let idx = cqe.user_data() as usize;
                if idx < results.len() {
                    results[idx] = if cqe.result() < 0 {
                        Err(std::io::Error::from_raw_os_error(-cqe.result()))
                    } else {
                        Ok(cqe.result() as usize)
                    };
                }
                reaped += 1;
                if reaped == submitted {
                    break;
                }
            }

            // anything that never made it into the ring is reported as unwritten
            for r in results.iter_mut().skip(submitted) {
                *r = Ok(0);
            }
            results
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    /// Stub so the crate builds on non-Linux; callers fall back to sequential writes.
    pub struct BatchWriter;

    impl BatchWriter {
        pub fn new(_capacity: usize) -> Option<Self> {
            None
        }
        pub fn capacity(&self) -> usize {
            0
        }
        pub fn write_all(
            &mut self,
            _items: &[(std::os::fd::RawFd, &[u8])],
        ) -> Vec<std::io::Result<usize>> {
            Vec::new()
        }
    }
}

pub use imp::BatchWriter;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        io::Read,
        net::{TcpListener, TcpStream},
        os::fd::AsRawFd,
    };

    /// Two sockets, one submission, both payloads arrive intact.
    #[test]
    fn writes_to_several_sockets_in_one_submission() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let accepted = std::thread::spawn(move || {
            let mut out = Vec::new();
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                out.push(std::thread::spawn(move || {
                    let mut buf = vec![0u8; 64];
                    let n = sock.read(&mut buf).unwrap();
                    buf.truncate(n);
                    buf
                }));
            }
            out
        });

        let a = TcpStream::connect(addr).unwrap();
        let b = TcpStream::connect(addr).unwrap();
        let readers = accepted.join().unwrap();

        let mut writer = BatchWriter::new(4).expect("io_uring available on linux");
        let first = b"hello from the first socket";
        let second = b"and the second one";
        let results = writer.write_all(&[(a.as_raw_fd(), first), (b.as_raw_fd(), second)]);

        assert_eq!(results.len(), 2);
        assert_eq!(*results[0].as_ref().unwrap(), first.len());
        assert_eq!(*results[1].as_ref().unwrap(), second.len());

        let mut got: Vec<Vec<u8>> = readers.into_iter().map(|h| h.join().unwrap()).collect();
        got.sort();
        let mut want = vec![first.to_vec(), second.to_vec()];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn a_closed_socket_reports_an_error_without_affecting_the_others() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            sock
        });
        let good = TcpStream::connect(addr).unwrap();
        let _held = accepted.join().unwrap();

        let mut writer = BatchWriter::new(4).unwrap();
        let payload = b"payload";
        // fd 2 is stderr: writable, but not a socket we own; -1 is invalid
        let results = writer.write_all(&[(good.as_raw_fd(), payload), (-1, payload)]);
        assert_eq!(*results[0].as_ref().unwrap(), payload.len());
        assert!(results[1].is_err(), "invalid fd must surface as an error");
    }
}
