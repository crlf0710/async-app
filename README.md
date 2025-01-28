# Async application logic entry and event loop spawning

This crate provides ergonomic approach to implement applications spawning event loops.
This covers applications using GUI & TUI (Screen rendering and input responding) /
Network Services / Audio Services and so on.
The `main` logic is implemented as an async function, and spawning event loops is
implemented as `Future`s that after yielded, returns a proxy object that has access to the
event loop. When yielded, the `main` future is also transfered and continue running on a new
thread, leaving the original thread to run the event loop.
To avoid pulling unnecessary dependencies, this crate doesn't provide any definitions for event
loops, so you usually need to use the definition from another crate, usually with the name `async-app-*`

The final code will be like:
```rust,ignore
use async_app_myeventloop::MyEventLoop;
// This macro below expands to some boilerplate to run main future
#[async_app::main]
async fn main(mut scope: async_app::Scope) {
    let proxy = scope
        .fork_and_run_event_loop::<MyEventLoop>()
        .await
        .expect("Failed to spawn event loop");
    // code below runs on the thread #2
    proxy.do_necessary_configuration();
    // thread #1's event loop is now modified with this configuration.
    
    // more application logic here

    // dropping `proxy` here will usually tell the event loop to quit somehow.
}
```