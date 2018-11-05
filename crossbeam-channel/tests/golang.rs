//! Tests copied from Go and manually rewritten in Rust.
//!
//! # Copyright
//!
//! The original tests can be found in the Go source distribution.
//!
//! Authors: https://golang.org/AUTHORS
//! License: https://golang.org/LICENSE
//! Source: https://github.com/golang/go
//!
//! ```text
//! Copyright (c) 2009 The Go Authors. All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are
//! met:
//!
//!    * Redistributions of source code must retain the above copyright
//! notice, this list of conditions and the following disclaimer.
//!    * Redistributions in binary form must reproduce the above
//! copyright notice, this list of conditions and the following disclaimer
//! in the documentation and/or other materials provided with the
//! distribution.
//!    * Neither the name of Google Inc. nor the names of its
//! contributors may be used to endorse or promote products derived from
//! this software without specific prior written permission.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
//! "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
//! LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
//! A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
//! OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//! SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
//! LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
//! DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
//! THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
//! (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//! OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//! ```

#[macro_use]
extern crate crossbeam_channel;
extern crate parking_lot;

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Select, Sender};
use parking_lot::{Condvar, Mutex};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

struct Chan<T> {
    inner: Arc<Mutex<ChanInner<T>>>,
}

struct ChanInner<T> {
    s: Option<Sender<T>>,
    r: Receiver<T>,
}

impl<T> Clone for Chan<T> {
    fn clone(&self) -> Chan<T> {
        Chan {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Chan<T> {
    fn send(&self, msg: T) {
        let s = self
            .inner
            .lock()
            .s
            .as_ref()
            .expect("sending into closed channel")
            .clone();
        let _ = s.send(msg);
    }

    fn try_recv(&self) -> Option<T> {
        let r = self.inner.lock().r.clone();
        r.try_recv().ok()
    }

    fn recv(&self) -> Option<T> {
        let r = self.inner.lock().r.clone();
        r.recv().ok()
    }

    fn close(&self) {
        self.inner.lock().s.take().expect("channel already closed");
    }

    fn rx(&self) -> Receiver<T> {
        self.inner.lock().r.clone()
    }

    fn tx(&self) -> Sender<T> {
        match self.inner.lock().s.as_ref() {
            None => {
                let (s, r) = bounded(0);
                std::mem::forget(r);
                s
            }
            Some(s) => s.clone(),
        }
    }
}

impl<T> Iterator for Chan<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv()
    }
}

impl<'a, T> IntoIterator for &'a Chan<T> {
    type Item = T;
    type IntoIter = Chan<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.clone()
    }
}

fn make<T>(cap: usize) -> Chan<T> {
    let (s, r) = bounded(cap);
    Chan {
        inner: Arc::new(Mutex::new(ChanInner { s: Some(s), r })),
    }
}

#[derive(Clone)]
struct WaitGroup(Arc<WaitGroupInner>);

struct WaitGroupInner {
    cond: Condvar,
    count: Mutex<i32>,
}

impl WaitGroup {
    fn new() -> WaitGroup {
        WaitGroup(Arc::new(WaitGroupInner {
            cond: Condvar::new(),
            count: Mutex::new(0),
        }))
    }

    fn add(&self, delta: i32) {
        let mut count = self.0.count.lock();
        *count += delta;
        assert!(*count >= 0);
        self.0.cond.notify_all();
    }

    fn done(&self) {
        self.add(-1);
    }

    fn wait(&self) {
        let mut count = self.0.count.lock();
        while *count > 0 {
            self.0.cond.wait(&mut count);
        }
    }
}

struct Defer<F: FnOnce()> {
    f: Option<Box<F>>,
}

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        let f = self.f.take().unwrap();
        let mut f = Some(f);
        let mut f = move || f.take().unwrap()();
        f();
    }
}

macro_rules! defer {
    ($body:expr) => {
        let _defer = Defer {
            f: Some(Box::new(|| $body)),
        };
    };
}

macro_rules! go {
    (@parse ref $v:ident, $($tail:tt)*) => {{
        let ref $v = $v;
        go!(@parse $($tail)*)
    }};
    (@parse move $v:ident, $($tail:tt)*) => {{
        let $v = $v;
        go!(@parse $($tail)*)
    }};
    (@parse $v:ident, $($tail:tt)*) => {{
        let $v = $v.clone();
        go!(@parse $($tail)*)
    }};
    (@parse $body:expr) => {
        ::std::thread::spawn(move || {
            let res = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                $body
            }));
            if res.is_err() {
                eprintln!("goroutine panicked: {:?}", res);
                ::std::process::abort();
            }
        })
    };
    (@parse $($tail:tt)*) => {
        compile_error!("invalid `go!` syntax")
    };
    ($($tail:tt)*) => {{
        go!(@parse $($tail)*)
    }};
}

// https://github.com/golang/go/blob/master/test/chan/doubleselect.go
mod doubleselect {
    use super::*;

    const ITERATIONS: i32 = 10_000;

    fn sender(n: i32, c1: Chan<i32>, c2: Chan<i32>, c3: Chan<i32>, c4: Chan<i32>) {
        defer! { c1.close() }
        defer! { c2.close() }
        defer! { c3.close() }
        defer! { c4.close() }

        for i in 0..n {
            select! {
                send(c1.tx(), i) -> _ => {}
                send(c2.tx(), i) -> _ => {}
                send(c3.tx(), i) -> _ => {}
                send(c4.tx(), i) -> _ => {}
            }
        }
    }

    fn mux(out: Chan<i32>, inp: Chan<i32>, done: Chan<bool>) {
        for v in inp {
            out.send(v);
        }
        done.send(true);
    }

    fn recver(inp: Chan<i32>) {
        let mut seen = HashMap::new();

        for v in &inp {
            if seen.contains_key(&v) {
                panic!("got duplicate value for {}", v);
            }
            seen.insert(v, true);
        }
    }

    #[test]
    fn main() {
        let c1 = make::<i32>(0);
        let c2 = make::<i32>(0);
        let c3 = make::<i32>(0);
        let c4 = make::<i32>(0);
        let done = make::<bool>(0);
        let cmux = make::<i32>(0);

        go!(c1, c2, c3, c4, sender(ITERATIONS, c1, c2, c3, c4));
        go!(cmux, c1, done, mux(cmux, c1, done));
        go!(cmux, c2, done, mux(cmux, c2, done));
        go!(cmux, c3, done, mux(cmux, c3, done));
        go!(cmux, c4, done, mux(cmux, c4, done));
        go!(done, cmux, {
            done.recv();
            done.recv();
            done.recv();
            done.recv();
            cmux.close();
        });
        recver(cmux);
    }
}

// https://github.com/golang/go/blob/master/test/chan/fifo.go
mod fifo {
    use super::*;

    const N: i32 = 10;

    #[test]
    fn asynch_fifo() {
        let ch = make::<i32>(N as usize);
        for i in 0..N {
            ch.send(i);
        }
        for i in 0..N {
            if ch.recv() != Some(i) {
                panic!("bad receive");
            }
        }
    }

    fn chain(ch: Chan<i32>, val: i32, inp: Chan<i32>, out: Chan<i32>) {
        inp.recv();
        if ch.recv() != Some(val) {
            panic!(val);
        }
        out.send(1);
    }

    #[test]
    fn synch_fifo() {
        let ch = make::<i32>(0);
        let mut inp = make::<i32>(0);
        let start = inp.clone();

        for i in 0..N {
            let out = make::<i32>(0);
            go!(ch, i, inp, out, chain(ch, i, inp, out));
            inp = out;
        }

        start.send(0);
        for i in 0..N {
            ch.send(i);
        }
        inp.recv();
    }
}

// https://github.com/golang/go/blob/master/test/chan/nonblock.go
mod nonblock {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/select.go
mod select {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/select2.go
mod select2 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/select3.go
mod select3 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/select4.go
mod select4 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/select5.go
mod select5 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/select6.go
mod select6 {
    // TODO
    use super::*;

    #[test]
    fn main() {
        let c1 = make::<bool>(0);
        let c2 = make::<bool>(0);
        let c3 = make::<bool>(0);

        go!(c1, c1.recv());
        go!(c1, c2, c3, {
            select! {
                recv(c1.rx()) -> _ => panic!("dummy"),
                recv(c2.rx()) -> _ => c3.send(true),
            }
            c1.recv();
        });
        go!(c2, c2.send(true));

        c3.recv();
        c1.send(true);
        c1.send(true);
    }
}

// https://github.com/golang/go/blob/master/test/chan/select7.go
mod select7 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/sieve1.go
mod sieve1 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/sieve2.go
mod sieve2 {
    // TODO
}

// https://github.com/golang/go/blob/master/test/chan/zerosize.go
mod zerosize {
    use super::*;

    #[test]
    fn zero_size_struct() {
        struct ZeroSize;
        let _ = make::<ZeroSize>(0);
    }

    #[test]
    fn zero_size_array() {
        let _ = make::<[u8; 0]>(0);
    }
}

// https://github.com/golang/go/blob/master/src/runtime/chan_test.go
mod chan_test {
    use super::*;

    #[test]
    fn test_chan() {
        const N: i32 = 200;

        for cap in 0..N {
            {
                // Ensure that receive from empty chan blocks.
                let c = make::<i32>(cap as usize);

                let recv1 = Arc::new(Mutex::new(false));
                go!(c, recv1, {
                    c.recv();
                    *recv1.lock() = true;
                });

                let recv2 = Arc::new(Mutex::new(false));
                go!(c, recv2, {
                    c.recv();
                    *recv2.lock() = true;
                });

                thread::sleep(ms(1));

                if *recv1.lock() || *recv2.lock() {
                    panic!();
                }

                // Ensure that non-blocking receive does not block.
                select! {
                    recv(c.rx()) -> _ => panic!(),
                    default => {}
                }
                select! {
                    recv(c.rx()) -> _ => panic!(),
                    default => {}
                }

                c.send(0);
                c.send(0);
            }

            {
                // Ensure that send to full chan blocks.
                let c = make::<i32>(cap as usize);
                for i in 0..cap {
                    c.send(i);
                }

                let sent = Arc::new(Mutex::new(0));
                go!(sent, c, {
                    c.send(0);
                    *sent.lock() = 1;
                });

                thread::sleep(ms(1));

                if *sent.lock() != 0 {
                    panic!();
                }

                // Ensure that non-blocking send does not block.
                select! {
                    send(c.tx(), 0) -> _ => panic!(),
                    default => {}
                }
                c.recv();
            }

            {
                // Ensure that we receive 0 from closed chan.
                let c = make::<i32>(cap as usize);
                for i in 0..cap {
                    c.send(i);
                }
                c.close();

                for i in 0..cap {
                    let v = c.recv();
                    if v != Some(i) {
                        panic!();
                    }
                }

                if c.recv() != None {
                    panic!();
                }
                if c.try_recv() != None {
                    panic!();
                }
            }

            {
                // Ensure that close unblocks receive.
                let c = make::<i32>(cap as usize);
                let done = make::<bool>(0);

                go!(c, done, {
                    let v = c.try_recv();
                    done.send(v.is_some());
                });

                thread::sleep(ms(1));
                c.close();

                if !done.recv().unwrap() {
                    // panic!();
                }
            }

            {
                // Send 100 integers,
                // ensure that we receive them non-corrupted in FIFO order.
                let c = make::<i32>(cap as usize);
                go!(c, {
                    for i in 0..100 {
                        c.send(i);
                    }
                });
                for i in 0..100 {
                    if c.recv() != Some(i) {
                        panic!();
                    }
                }

                // Same, but using recv2.
                go!(c, {
                    for i in 0..100 {
                        c.send(i);
                    }
                });
                for i in 0..100 {
                    if c.recv() != Some(i) {
                        panic!();
                    }
                }
            }
        }
    }

    #[test]
    fn test_nonblock_recv_race() {
        const N: usize = 1000;

        for _ in 0..N {
            let c = make::<i32>(1);
            c.send(1);

            let t = go!(c, {
                select! {
                    recv(c.rx()) -> _ => {}
                    default => panic!("chan is not ready"),
                }
            });

            c.close();
            c.recv();
            t.join().unwrap();
        }
    }

    #[test]
    fn test_nonblock_select_race() {
        const N: usize = 1000;

        let done = make::<bool>(1);
        for _ in 0..N {
            let c1 = make::<i32>(1);
            let c2 = make::<i32>(1);
            c1.send(1);

            go!(c1, c2, done, {
                select! {
                    recv(c1.rx()) -> _ => {}
                    recv(c2.rx()) -> _ => {}
                    default => {
                        done.send(false);
                        return;
                    }
                }
                done.send(true);
            });

            c2.send(1);
            select! {
                recv(c1.rx()) -> _ => {}
                default => {}
            }
            if !done.recv().unwrap() {
                panic!("no chan is ready");
            }
        }
    }

    #[test]
    fn test_nonblock_select_race2() {
        const N: usize = 1000;

        let done = make::<bool>(1);
        for _ in 0..N {
            let c1 = make::<i32>(1);
            let c2 = make::<i32>(0);
            c1.send(1);

            go!(c1, c2, done, {
                select! {
                    recv(c1.rx()) -> _ => {}
                    recv(c2.rx()) -> _ => {}
                    default => {
                        done.send(false);
                        return;
                    }
                }
                done.send(true);
            });

            c2.close();
            select! {
                recv(c1.rx()) -> _ => {}
                default => {}
            }
            if !done.recv().unwrap() {
                panic!("no chan is ready");
            }
        }
    }

    #[test]
    fn test_self_select() {
        // Ensure that send/recv on the same chan in select
        // does not crash nor deadlock.

        for &cap in &[0, 10] {
            let wg = WaitGroup::new();
            wg.add(2);
            let c = make::<i32>(cap);

            for p in 0..2 {
                let p = p;
                go!(wg, p, c, {
                    defer! { wg.done() }
                    for i in 0..1000 {
                        if p == 0 || i % 2 == 0 {
                            select! {
                                send(c.tx(), p) -> _ => {}
                                recv(c.rx()) -> v => {
                                    if cap == 0 && v.ok() == Some(p) {
                                        panic!("self receive");
                                    }
                                }
                            }
                        } else {
                            select! {
                                recv(c.rx()) -> v => {
                                    if cap == 0 && v.ok() == Some(p) {
                                        panic!("self receive");
                                    }
                                }
                                send(c.tx(), p) -> _ => {}
                            }
                        }
                    }
                });
            }
            wg.wait();
        }
    }

    #[test]
    fn test_select_stress() {
        let c = vec![
            make::<i32>(0),
            make::<i32>(0),
            make::<i32>(2),
            make::<i32>(3),
        ];

        const N: usize = 10000;

        // There are 4 goroutines that send N values on each of the chans,
        // + 4 goroutines that receive N values on each of the chans,
        // + 1 goroutine that sends N values on each of the chans in a single select,
        // + 1 goroutine that receives N values on each of the chans in a single select.
        // All these sends, receives and selects interact chaotically at runtime,
        // but we are careful that this whole construct does not deadlock.
        let wg = WaitGroup::new();
        wg.add(10);

        for k in 0..4 {
            go!(k, c, wg, {
                for _ in 0..N {
                    c[k].send(0);
                }
                wg.done();
            });
            go!(k, c, wg, {
                for _ in 0..N {
                    c[k].recv();
                }
                wg.done();
            });
        }

        go!(c, wg, {
            let mut n = [0; 4];
            let mut c1 = c.iter().map(|c| Some(c.rx().clone())).collect::<Vec<_>>();

            for _ in 0..4 * N {
                let index = {
                    let mut sel = Select::new();
                    let mut opers = [!0; 4];
                    for &i in &[3, 2, 0, 1] {
                        if let Some(c) = &c1[i] {
                            opers[i] = sel.recv(c);
                        }
                    }

                    let oper = sel.select();
                    let mut index = !0;
                    for i in 0..4 {
                        if opers[i] == oper.index() {
                            index = i;
                            let _ = oper.recv(c1[i].as_ref().unwrap());
                            break;
                        }
                    }
                    index
                };

                n[index] += 1;
                if n[index] == N {
                    c1[index] = None;
                }
            }
            wg.done();
        });

        go!(c, wg, {
            let mut n = [0; 4];
            let mut c1 = c.iter().map(|c| Some(c.tx().clone())).collect::<Vec<_>>();

            for _ in 0..4 * N {
                let index = {
                    let mut sel = Select::new();
                    let mut opers = [!0; 4];
                    for &i in &[0, 1, 2, 3] {
                        if let Some(c) = &c1[i] {
                            opers[i] = sel.send(c);
                        }
                    }

                    let oper = sel.select();
                    let mut index = !0;
                    for i in 0..4 {
                        if opers[i] == oper.index() {
                            index = i;
                            let _ = oper.send(c1[i].as_ref().unwrap(), 0);
                            break;
                        }
                    }
                    index
                };

                n[index] += 1;
                if n[index] == N {
                    c1[index] = None;
                }
            }
            wg.done();
        });

        wg.wait();
    }

    #[test]
    fn test_select_fairness() {
        const TRIALS: usize = 10000;

        let c1 = make::<u8>(TRIALS + 1);
        let c2 = make::<u8>(TRIALS + 1);

        for _ in 0..TRIALS + 1 {
            c1.send(1);
            c2.send(2);
        }

        let c3 = make::<u8>(0);
        let c4 = make::<u8>(0);
        let out = make::<u8>(0);
        let done = make::<u8>(0);
        let wg = WaitGroup::new();

        wg.add(1);
        go!(wg, c1, c2, c3, c4, out, done, {
            defer! { wg.done() };
            loop {
                let b;
                select! {
                    recv(c3.rx()) -> m => b = m.unwrap(),
                    recv(c4.rx()) -> m => b = m.unwrap(),
                    recv(c1.rx()) -> m => b = m.unwrap(),
                    recv(c2.rx()) -> m => b = m.unwrap(),
                }
                select! {
                    send(out.tx(), b) -> _ => {}
                    recv(done.rx()) -> _ => return,
                }
            }
        });

        let (mut cnt1, mut cnt2) = (0, 0);
        for _ in 0..TRIALS {
            match out.recv() {
                Some(1) => cnt1 += 1,
                Some(2) => cnt2 += 1,
                b => panic!("unexpected value {:?} on channel", b),
            }
        }

        // If the select in the goroutine is fair,
        // cnt1 and cnt2 should be about the same value.
        // With 10,000 trials, the expected margin of error at
        // a confidence level of five nines is 4.4172 / (2 * Sqrt(10000)).

        let r = cnt1 as f64 / TRIALS as f64;
        let e = (r - 0.5).abs();

        if e > 4.4172 / (2.0 * (TRIALS as f64).sqrt()) {
            panic!(
                "unfair select: in {} trials, results were {}, {}",
                TRIALS, cnt1, cnt2,
            );
        }

        done.close();
        wg.wait();
    }

    #[test]
    fn test_chan_send_interface() {
        struct Mt;

        let c = make::<Box<Any>>(1);
        c.send(Box::new(Mt));

        select! {
            send(c.tx(), Box::new(Mt)) -> _ => {}
            default => {}
        }

        select! {
            send(c.tx(), Box::new(Mt)) -> _ => {}
            send(c.tx(), Box::new(Mt)) -> _ => {}
            default => {}
        }
    }

    #[test]
    fn test_pseudo_random_send() {
        const N: usize = 100;

        for cap in 0..N {
            let c = make::<i32>(cap);
            let l = Arc::new(Mutex::new(vec![0i32; N]));
            let done = make::<bool>(0);

            go!(c, done, l, {
                let mut l = l.lock();
                for i in 0..N {
                    thread::yield_now();
                    l[i] = c.recv().unwrap();
                }
                done.send(true);
            });

            for _ in 0..N {
                select! {
                    send(c.tx(), 1) -> _ => {}
                    send(c.tx(), 0) -> _ => {}
                }
            }
            done.recv();

            let mut n0 = 0;
            let mut n1 = 0;
            for &i in l.lock().iter() {
                n0 += (i + 1) % 2;
                n1 += i;
            }

            if n0 <= N as i32 / 10 || n1 <= N as i32 / 10 {
                panic!(
                    "Want pseudorandom, got {} zeros and {} ones (chan cap {})",
                    n0, n1, cap,
                );
            }
        }
    }

    #[test]
    fn test_multi_consumer() {
        const NWORK: usize = 23;
        const NITER: usize = 271828;

        let pn = [2, 3, 7, 11, 13, 17, 19, 23, 27, 31];

        let q = make::<i32>(NWORK * 3);
        let r = make::<i32>(NWORK * 3);

        let wg = WaitGroup::new();
        for i in 0..NWORK {
            wg.add(1);
            let w = i;
            go!(q, r, wg, pn, {
                for v in &q {
                    if pn[w % pn.len()] == v {
                        thread::yield_now();
                    }
                    r.send(v);
                }
                wg.done();
            });
        }

        let expect = Arc::new(Mutex::new(0));
        go!(q, r, expect, wg, pn, {
            for i in 0..NITER {
                let v = pn[i % pn.len()];
                *expect.lock() += v;
                q.send(v);
            }
            q.close();
            wg.wait();
            r.close();
        });

        let mut n = 0;
        let mut s = 0;
        for v in &r {
            n += 1;
            s += v;
        }

        if n != NITER || s != *expect.lock() {
            panic!();
        }
    }

    #[test]
    fn test_select_duplicate_channel() {
        // This test makes sure we can queue a G on
        // the same channel multiple times.
        let c = make::<i32>(0);
        let d = make::<i32>(0);
        let e = make::<i32>(0);

        go!(c, d, e, {
            select! {
                recv(c.rx()) -> _ => {}
                recv(d.rx()) -> _ => {}
                recv(e.rx()) -> _ => {}
            }
            e.send(9);
        });
        thread::sleep(ms(1));

        go!(c, c.recv());
        thread::sleep(ms(1));

        d.send(7);
        e.recv();
        c.send(8);
    }
}

// https://github.com/golang/go/blob/master/test/closedchan.go
mod closedchan {
    // TODO
}

// https://github.com/golang/go/blob/master/src/runtime/chanbarrier_test.go
mod chanbarrier_test {
    // TODO
}

// https://github.com/golang/go/blob/master/src/runtime/race/testdata/chan_test.go
mod race_chan_test {
    // TODO
}

// https://github.com/golang/go/blob/master/test/ken/chan.go
mod chan {
    // TODO
}

// https://github.com/golang/go/blob/master/test/ken/chan1.go
mod chan1 {
    // TODO
}
