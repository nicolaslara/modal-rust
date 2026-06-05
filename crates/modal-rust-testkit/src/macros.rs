//! `mock_unimplemented!` — emit a default `Status::unimplemented` impl for every
//! `ModalClient` RPC the mock does NOT hand-write (the ~184 the SDK never calls).
//!
//! KEY FINDING (proven in the spike): the generated server trait is
//! `#[async_trait]` (boxed-future desugaring). An attribute macro on the impl
//! (`#[tonic::async_trait]`) cannot see THROUGH an unexpanded `macro_rules!`
//! invocation, so plain `async fn`s emitted by this macro would never get the
//! async-trait lifetime rewrite → `E0195` ×189. The fix: emit the ALREADY-DESUGARED
//! form here — a `fn` returning a pinned boxed `Future` with the explicit
//! `'async_trait` lifetime + `where` bounds, exactly what `#[async_trait]` produces.
//! The hand-written RPCs stay normal `async fn` inside the `#[tonic::async_trait]`
//! impl; the macro arms are invoked in the SAME impl and pass through untouched.
//!
//! - `unary name(ReqTy) -> RespTy;` → one desugared async-trait method.
//! - `stream name[AssocStreamTy](ReqTy) -> ItemTy;` → the associated stream type
//!   bound to a concrete boxed empty stream AND the desugared method.
//!
//! Inside the impl the message paths resolve through `crate::proto::api`; arms are
//! written with the fully-qualified `$crate::proto::api::Foo` so they need no
//! `use` in scope.
macro_rules! mock_unimplemented {
    (
        $( unary $u_name:ident ( $u_req:ty ) -> $u_resp:ty ; )*
        $( stream $s_name:ident [ $s_assoc:ident ] ( $s_req:ty ) -> $s_item:ty ; )*
    ) => {
        $(
            fn $u_name<'life0, 'async_trait>(
                &'life0 self,
                _request: ::tonic::Request<$u_req>,
            ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<
                Output = ::std::result::Result<::tonic::Response<$u_resp>, ::tonic::Status>
            > + ::core::marker::Send + 'async_trait>>
            where 'life0: 'async_trait, Self: 'async_trait {
                Box::pin(async move {
                    Err(::tonic::Status::unimplemented(concat!(
                        "mock: ", stringify!($u_name), " is not implemented"
                    )))
                })
            }
        )*
        $(
            // A concrete, Send + 'static empty stream of the right item type.
            type $s_assoc = ::core::pin::Pin<Box<
                dyn ::tonic::codegen::tokio_stream::Stream<
                    Item = ::std::result::Result<$s_item, ::tonic::Status>
                > + ::core::marker::Send + 'static
            >>;
            fn $s_name<'life0, 'async_trait>(
                &'life0 self,
                _request: ::tonic::Request<$s_req>,
            ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<
                Output = ::std::result::Result<::tonic::Response<Self::$s_assoc>, ::tonic::Status>
            > + ::core::marker::Send + 'async_trait>>
            where 'life0: 'async_trait, Self: 'async_trait {
                Box::pin(async move {
                    Err(::tonic::Status::unimplemented(concat!(
                        "mock: ", stringify!($s_name), " is not implemented"
                    )))
                })
            }
        )*
    };
}

pub(crate) use mock_unimplemented;
