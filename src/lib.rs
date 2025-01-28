//! This crate provides ergonomic approach to implement applications spawning event loops.
//!
//! This covers applications using GUI & TUI (Screen rendering and input responding) /
//! Network Services / Audio Services and so on.
//! The `main` logic is implemented as an async function, and spawning event loops is
//! implemented as `Future`s that after yielded, returns a proxy object that has access to the
//! event loop. When yielded, the `main` future is also transfered and continue running on a new
//! thread, leaving the original thread to run the event loop.
//!
//! To avoid pulling unnecessary dependencies, this crate doesn't provide any definitions for event
//! loops, so you usually need to use the definition from another crate, usually with the name `async-app-*`
//! 
//! The final code will be like:
//!
//! ```rust,ignore
//! use async_app_myeventloop::MyEventLoop;
//!
//! // This macro below expands to some boilerplate to run main future
//! #[async_app::main]
//! async fn main(mut scope: async_app::Scope) {
//!     let proxy = scope
//!         .fork_and_run_event_loop::<MyEventLoop>()
//!         .await
//!         .expect("Failed to spawn event loop");
//!     // code below runs on the thread #2
//!     proxy.do_necessary_configuration();
//!     // thread #1's event loop is now modified with this configuration.
//!     
//!     // more application logic here
//! 
//!     // dropping `proxy` here will usually tell the event loop to quit somehow.
//! }
//!
//! ```

use std::cell::Cell;
use std::future::Future;
use std::mem;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};
use std::thread;

/// The macro supplying startup boilerplate
/// 
/// Use it on async `main` function like this:
/// ```rust,ignore
/// #[async_app::main]
/// async fn main(mut scope: async_app::Scope) {
///     //...
/// }
/// ```
pub use async_app_macros::main;

/// The scope handle on which one can spawn event loops
#[non_exhaustive]
pub struct Scope {}

/// Failing to spawn event loop, which rarely happens
#[derive(Debug)]
pub struct ForkFailError;

impl Scope {
    pub fn fork_and_run_event_loop<T: EventLoopDesc>(
        &mut self,
    ) -> impl Future<Output = Result<T::ProxyType, ForkFailError>> + Send {
        self.fork_and_run_event_loop_impl(|| T::build_service_and_proxy())
    }

    pub fn fork_and_run_event_loop_with_arg<T: EventLoopWithArgsDesc>(
        &mut self,
        args: <T as EventLoopWithArgsDesc>::Args,
    ) -> impl Future<Output = Result<T::ProxyType, ForkFailError>> + Send {
        self.fork_and_run_event_loop_impl(|| T::build_service_and_proxy_with_args(args))
    }

    fn fork_and_run_event_loop_impl<R, F>(
        &mut self,
        f: F,
    ) -> impl Future<Output = Result<R, ForkFailError>> + Send
    where
        R: Unpin + Send,
        F: FnOnce() -> (Box<dyn FnOnce()>, R) + Unpin,
    {
        enum SpaceLeap<R, F>
        where
            F: FnOnce() -> (Box<dyn FnOnce()>, R),
        {
            Preparation(Option<fragile::Fragile<F>>),
            Launched {
                launching_thread: thread::ThreadId,
                proxy_value: R,
            },
            Landed,
            Aborted,
        }

        impl<R, F> Future for SpaceLeap<R, F>
        where
            R: Unpin,
            F: FnOnce() -> (Box<dyn FnOnce()>, R) + Unpin,
        {
            type Output = Result<R, ForkFailError>;

            fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
                let this = self.get_mut();
                match this {
                    SpaceLeap::Preparation(f) => {
                        let (service, proxy) = f
                            .take()
                            .expect("Try to take builder function multiple times")
                            .into_inner()();
                        EVENT_LOOP_CHOICE.set(EventLoopChoice::ForkAndRunEventLoop(service));
                        *this = SpaceLeap::Launched {
                            launching_thread: thread::current().id(),
                            proxy_value: proxy,
                        };
                        Poll::Pending
                    }
                    SpaceLeap::Launched {
                        launching_thread, ..
                    } => {
                        let current_thread_id = thread::current().id();
                        if current_thread_id == *launching_thread {
                            // suspicious wakening or aborted?
                            if let Some(_failed) =
                                EVENT_LOOP_CHOICE.take_filter(|cur_choice| match cur_choice {
                                    EventLoopChoice::ForkFailureAndNeedRepoll => {
                                        ControlFlow::Break(())
                                    }
                                    _ => ControlFlow::Continue(cur_choice),
                                })
                            {
                                *this = SpaceLeap::Aborted;
                                cx.waker().wake_by_ref();
                                return Poll::Ready(Err(ForkFailError));
                            }
                            return Poll::Pending;
                        }

                        let old_value = mem::replace(this, SpaceLeap::Landed {});
                        let SpaceLeap::Launched {
                            proxy_value,
                            launching_thread: _,
                        } = old_value
                        else {
                            unreachable!()
                        };
                        cx.waker().wake_by_ref();
                        Poll::Ready(Ok(proxy_value))
                    }
                    SpaceLeap::Landed => unreachable!(),
                    SpaceLeap::Aborted => unreachable!(),
                }
            }
        }
        SpaceLeap::<R, F>::Preparation(Some(fragile::Fragile::new(f)))
    }
}

/// Definition of a event loop with default arguments
pub trait EventLoopDesc {
    type ProxyType: Unpin + Send;

    fn build_service_and_proxy() -> (Box<dyn FnOnce()>, Self::ProxyType);
}

/// Definition of a event loop with customized arguments
pub trait EventLoopWithArgsDesc {
    type Args: Unpin;
    type ProxyType: Unpin + Send;

    fn build_service_and_proxy_with_args(args: Self::Args) -> (Box<dyn FnOnce()>, Self::ProxyType);
}

impl<T> EventLoopDesc for T
where
    T: EventLoopWithArgsDesc,
    <T as EventLoopWithArgsDesc>::Args: Default,
{
    type ProxyType = <T as EventLoopWithArgsDesc>::ProxyType;
    fn build_service_and_proxy() -> (Box<dyn FnOnce()>, Self::ProxyType) {
        T::build_service_and_proxy_with_args(Default::default())
    }
}

thread_local! {
    static EVENT_LOOP_CHOICE: Cell<EventLoopChoice> =
        const { Cell::new(EventLoopChoice::Repoll) };
}

#[derive(Default)]
enum EventLoopChoice {
    #[default]
    Repoll,
    ForkAndRunEventLoop(Box<dyn FnOnce()>),
    #[allow(dead_code)]
    ForkFailureAndNeedRepoll,
}

#[doc(hidden)]
pub fn entryscope() -> Scope {
    Scope {}
}

struct UnparkWaker(thread::Thread);

impl task::Wake for UnparkWaker {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.unpark();
    }
}

#[doc(hidden)]
pub fn entrypoint<R: Send + 'static>(
    mut main_future: Pin<Box<impl Future<Output = R> + Send + 'static>>,
) -> R {
    let waker = task::Waker::from(Arc::new(UnparkWaker(thread::current())));
    let mut ctx = task::Context::from_waker(&waker);
    let event_loop_to_run = loop {
        if let Poll::Ready(r) = main_future.as_mut().poll(&mut ctx) {
            return r;
        }

        let Some(event_loop_to_run) =
            EVENT_LOOP_CHOICE.take_filter(|cur_choice| match cur_choice {
                EventLoopChoice::Repoll | EventLoopChoice::ForkFailureAndNeedRepoll => {
                    ControlFlow::Continue(cur_choice)
                }
                EventLoopChoice::ForkAndRunEventLoop(f) => ControlFlow::Break(f),
            })
        else {
            thread::park();
            continue;
        };

        break event_loop_to_run;
    };

    // move to new thread
    let waker = waker.clone();
    let new_main_thread_join_handle = thread::Builder::new()
        .spawn(move || {
            waker.wake_by_ref();
            entrypoint(main_future)
        })
        .expect("Failed to create thread");

    event_loop_to_run();

    new_main_thread_join_handle
        .join()
        .expect("Failed to join created thread")
}

use utils::TakeFilter;

mod utils {
    use std::cell::Cell;
    use std::{ops::ControlFlow, thread};

    pub(crate) trait TakeFilter<T> {
        fn take_filter<R, F>(&'static self, f: F) -> Option<R>
        where
            F: FnOnce(T) -> ControlFlow<R, T>;
    }

    impl<T> TakeFilter<T> for thread::LocalKey<Cell<T>>
    where
        T: Default,
    {
        fn take_filter<R, F>(&'static self, f: F) -> Option<R>
        where
            F: FnOnce(T) -> ControlFlow<R, T>,
        {
            let v = f(self.take());
            match v {
                ControlFlow::Continue(v) => {
                    self.set(v);
                    None
                }
                ControlFlow::Break(r) => Some(r),
            }
        }
    }
}
