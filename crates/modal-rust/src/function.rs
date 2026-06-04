//! The [`Function`] handle: real in-process `.local()` plus the live
//! `.remote()`/`.spawn()`/`.map()` async RUN-path surface.
//!
//! `.local()` dispatches through the FROZEN [`Registry`](crate::Registry) exactly
//! as the runner would, minus the subprocess: `serde_json::to_vec(&input)` →
//! `(HandlerFn)(&bytes)` → `serde_json::from_slice(&out)`. So
//! `add(AddInput{a:40,b:2})` yields `AddOutput{sum:42}`, identical to the CLI
//! runner.

use crate::{Error, HandlerFn, Result};

/// A handle to a single registered entrypoint of an [`App`](crate::App).
///
/// Obtained via [`App::function`](crate::App::function). The handler is resolved
/// at construction (cheap, `Copy`); an unknown name is reported as
/// [`Error::UnknownEntrypoint`] when an operation is actually invoked.
pub struct Function<'a> {
    pub(crate) app: &'a crate::App,
    /// Owned: `App::function` takes `&str`.
    pub(crate) name: String,
    /// `Some` if the entrypoint was found in the registry, `None` if unknown.
    pub(crate) handler: Option<HandlerFn>,
}

impl<'a> Function<'a> {
    /// Run the registered handler IN-PROCESS via the FROZEN [`Registry`](crate::Registry):
    /// serialize `input` to JSON, invoke the [`HandlerFn`], deserialize the JSON
    /// output to `Out`. Zero Modal, zero network. Mirrors Modal Python's
    /// `Function.local()` = `raw_f(*args)`.
    ///
    /// The double JSON round-trip is intentional and correct: input → JSON →
    /// handler's own `codec::decode` → `In`; handler's `codec::encode` → JSON →
    /// `from_slice` → `Out`. This is identical to running the runner without a
    /// subprocess.
    ///
    /// # Errors
    /// - [`Error::UnknownEntrypoint`] if the name is not registered.
    /// - [`Error::Encode`] if `input` fails to serialize to JSON.
    /// - [`Error::Runner`] wrapping any of the five frozen runtime failure kinds
    ///   (handler-side decode of `In`, function body `Err`, encode of `Out`, panic).
    /// - [`Error::Decode`] if the handler's JSON output does not match `Out`.
    pub fn local<In, Out>(&self, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let handler = self.handler.ok_or_else(|| self.unknown())?;
        let bytes = serde_json::to_vec(&input).map_err(Error::Encode)?;
        let out = handler(&bytes).map_err(Error::Runner)?;
        serde_json::from_slice(&out).map_err(Error::Decode)
    }

    /// Run the function body REMOTELY on Modal (the RUN path), returning the typed
    /// output with the SAME semantics as [`Function::local`].
    ///
    /// Requires a connected App ([`App::connect`](crate::App::connect)) — otherwise
    /// [`Error::NotConnected`]. On first call the App ensures the wrapper function
    /// exists (uploads the source crate as a mount, builds the run image, creates +
    /// publishes the FILE-mode function); subsequent calls reuse the memoized
    /// `function_id`. The user's Rust crate is `cargo build`-ed IN THE FUNCTION
    /// BODY at invoke time (the run boundary), then `modal_runner` execs the
    /// registered handler and emits the same JSON envelope `.local()` produces.
    ///
    /// # Errors
    /// - [`Error::NotConnected`] if the App was not connected.
    /// - [`Error::Encode`] if `input` fails to serialize to JSON.
    /// - [`Error::Sdk`] for any control-plane / upload / build / invoke failure
    ///   (including a remote `cargo build` failure, surfaced with its traceback).
    /// - [`Error::Runner`] wrapping the frozen five-kind taxonomy (identical to
    ///   `.local()`) when the handler itself reports a structured failure.
    /// - [`Error::Decode`] if the envelope / output does not match `Out`.
    pub async fn remote<In, Out>(&self, input: In) -> Result<Out>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
    {
        let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
        let envelope = self.app.remote_invoke(&self.name, input_json).await?;
        crate::remote::parse_envelope::<Out>(&envelope)
    }

    /// Fire-and-forget spawn (the RUN path): enqueue ONE input on Modal and return
    /// a [`FunctionCall`] handle carrying the `function_call_id` IMMEDIATELY, without
    /// waiting for the output. Fetch the result later with [`FunctionCall::get`].
    ///
    /// Mirrors Modal Python's `Function.spawn`. The FIRST spawn on a fresh App
    /// ensures the wrapper function exists (upload + create + publish), exactly as
    /// `.remote()`; the spawned input pays the cold in-body `cargo build` when its
    /// container first handles it (covered by the `get` poll deadline).
    ///
    /// # Errors
    /// - [`Error::NotConnected`] if the App was not connected.
    /// - [`Error::Encode`] if `input` fails to serialize to JSON.
    /// - [`Error::Sdk`] for any control-plane / upload / enqueue failure.
    pub async fn spawn<In>(&self, input: In) -> Result<FunctionCall<'a>>
    where
        In: serde::Serialize,
    {
        let input_json = serde_json::to_string(&input).map_err(Error::Encode)?;
        let function_call_id = self.app.remote_spawn(&self.name, input_json).await?;
        Ok(FunctionCall {
            app: self.app,
            function_call_id,
        })
    }

    /// Fan-out over many inputs (the RUN path): enqueue N inputs under ONE map call
    /// and collect the N typed outputs in INPUT ORDER (Modal tags each output with
    /// its input ordinal; the SDK reassembles by ordinal), running across containers
    /// in parallel. Each output decodes with the SAME semantics as
    /// [`Function::local`]/[`Function::remote`].
    ///
    /// Mirrors Modal Python's `Function.map`. Fail-fast: the first remote failure
    /// surfaces immediately. The FIRST map on a fresh App ensures the wrapper exists,
    /// exactly as `.remote()`.
    ///
    /// # Errors
    /// - [`Error::NotConnected`] if the App was not connected.
    /// - [`Error::Encode`] if any input fails to serialize to JSON.
    /// - [`Error::Sdk`] for any control-plane / upload / enqueue / poll failure.
    /// - [`Error::Runner`] / [`Error::Decode`] per output, identical to `.remote()`.
    pub async fn map<In, Out, I>(&self, inputs: I) -> Result<Vec<Out>>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
        I: IntoIterator<Item = In>,
    {
        let inputs_json = inputs
            .into_iter()
            .map(|i| serde_json::to_string(&i).map_err(Error::Encode))
            .collect::<Result<Vec<_>>>()?;
        let envelopes = self.app.remote_map(&self.name, inputs_json).await?;
        envelopes
            .iter()
            .map(|e| crate::remote::parse_envelope::<Out>(e))
            .collect()
    }

    /// Build the [`Error::UnknownEntrypoint`] for this handle, listing the App's
    /// known names.
    fn unknown(&self) -> Error {
        Error::UnknownEntrypoint {
            name: self.name.clone(),
            known: self.app.known_names(),
        }
    }
}

/// Handle returned by [`Function::spawn`], carrying the spawned call's
/// `function_call_id` and a borrow of the [`App`](crate::App) it was spawned on
/// (which owns the live control-plane client). Mirrors Modal Python's
/// `_FunctionCall`, which carries `(function_call_id, client)`.
///
/// The borrow ties the handle's lifetime to the App: hold the App alive (it owns
/// the ephemeral run) and call [`get`](FunctionCall::get) to fetch the result.
pub struct FunctionCall<'a> {
    app: &'a crate::App,
    function_call_id: String,
}

// Manual `Debug` (not derived): the `&App` field is not `Debug` and printing the
// live control-plane handle is noise — the call id is the useful identity.
impl std::fmt::Debug for FunctionCall<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FunctionCall")
            .field("function_call_id", &self.function_call_id)
            .finish_non_exhaustive()
    }
}

impl FunctionCall<'_> {
    /// The spawned call's `function_call_id` (Modal's handle for the queued call).
    pub fn function_call_id(&self) -> &str {
        &self.function_call_id
    }

    /// Await the spawned call's result (the RUN path): poll Modal's
    /// `FunctionGetOutputs` for this call's single output (index `0` — spawn enqueues
    /// one input) and decode it with the SAME semantics as
    /// [`Function::local`]/[`Function::remote`].
    ///
    /// `timeout` bounds the output poll. `None` uses the wrapper's container timeout
    /// plus a buffer (covering the cold in-body `cargo build` the spawned input may
    /// still be running).
    ///
    /// # Errors
    /// - [`Error::NotConnected`] if the App was not connected.
    /// - [`Error::Sdk`] for any poll failure or a no-output timeout.
    /// - [`Error::Runner`] wrapping the frozen five-kind taxonomy (identical to
    ///   `.local()`) when the handler reports a structured failure.
    /// - [`Error::Decode`] if the envelope / output does not match `Out`.
    pub async fn get<Out>(&self, timeout: Option<std::time::Duration>) -> Result<Out>
    where
        Out: serde::de::DeserializeOwned,
    {
        let envelope = self
            .app
            .remote_get(&self.function_call_id, 0, timeout)
            .await?;
        crate::remote::parse_envelope::<Out>(&envelope)
    }
}
