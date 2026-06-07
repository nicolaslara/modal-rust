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
    /// exists for this entrypoint's effective config (uploads the source crate as a
    /// mount, builds the run image, creates + publishes the FILE-mode function);
    /// later calls to the same entrypoint with the same config reuse the memoized
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
    /// ensures a config-keyed wrapper function exists (upload + create + publish),
    /// exactly as `.remote()`; the spawned input pays the cold in-body `cargo build`
    /// when its container first handles it (covered by the `get` poll deadline).
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

    /// Fan-out where each input is UNPACKED from a tuple/sequence (the RUN path):
    /// the map-family member for "spread each item into the call". Returns the N
    /// typed outputs in INPUT ORDER, exactly like [`map`](Function::map).
    ///
    /// Mirrors Modal Python's `Function.starmap`, where each input item is unpacked
    /// into the function's positional args. `modal-rust` functions take exactly ONE
    /// named-object input (PARITY §6: multi-arg is reserved), so here each item `In`
    /// IS that one input — you make it a tuple/sequence shape and the function
    /// receives the unpacked whole. With that single-arg framing `starmap` and
    /// [`map`](Function::map) share the wire path; `starmap` is the right name when
    /// each input is naturally a tuple, and the seam for true multi-arg later.
    ///
    /// # Errors
    /// Identical to [`map`](Function::map).
    pub async fn starmap<In, Out, I>(&self, inputs: I) -> Result<Vec<Out>>
    where
        In: serde::Serialize,
        Out: serde::de::DeserializeOwned,
        I: IntoIterator<Item = In>,
    {
        self.map::<In, Out, I>(inputs).await
    }

    /// Fan-out for SIDE EFFECTS (the RUN path): run the function over N inputs across
    /// containers, WAIT for them all to finish, and DISCARD the outputs — returning
    /// `()`. Use it to drive work whose result you do not need (writing to a Volume,
    /// notifying a webhook, etc.).
    ///
    /// Mirrors Modal Python's `Function.for_each` (`map` with the outputs dropped,
    /// parallel_map.py:1067). It is fail-fast like [`map`](Function::map): the first
    /// remote failure surfaces immediately. Unlike [`spawn_map`](Function::spawn_map)
    /// it BLOCKS until every input completes, so a returned `Ok(())` means all N ran.
    /// The caller never names an output type.
    ///
    /// # Errors
    /// Same as [`map`](Function::map), minus [`Error::Decode`]: outputs are decoded
    /// into [`serde::de::IgnoredAny`] (the success/failure envelope status still
    /// drives fail-fast, but the body is not interpreted).
    pub async fn for_each<In, I>(&self, inputs: I) -> Result<()>
    where
        In: serde::Serialize,
        I: IntoIterator<Item = In>,
    {
        // Drive the proven ordered-map collect, decoding each output into
        // `IgnoredAny` so no `Out` type is needed; then drop the collected vec. The
        // envelope's ok/err status is still honored per output (fail-fast).
        let _: Vec<serde::de::IgnoredAny> = self.map(inputs).await?;
        Ok(())
    }

    /// Fire-and-forget fan-out (the RUN path): enqueue N inputs under ONE map call
    /// and return a [`FunctionCall`] handle for that call IMMEDIATELY, WITHOUT
    /// waiting for any output. The fan-out runs on Modal regardless; results are not
    /// collected here.
    ///
    /// Mirrors Modal Python's `Function.spawn_map` (parallel_map.py:1220): spawn
    /// parallel execution and exit as soon as the inputs are created. It is the
    /// map-family analogue of [`spawn`](Function::spawn) (one input → N inputs) and
    /// the fire-and-forget counterpart of [`for_each`](Function::for_each) (which
    /// blocks). The FIRST spawn_map on a fresh App ensures the wrapper exists, exactly
    /// as `.remote()`.
    ///
    /// # Errors
    /// - [`Error::NotConnected`] if the App was not connected.
    /// - [`Error::Encode`] if any input fails to serialize to JSON.
    /// - [`Error::Sdk`] for any control-plane / upload / enqueue failure (including
    ///   zero inputs — spawn_map requires at least one).
    pub async fn spawn_map<In, I>(&self, inputs: I) -> Result<FunctionCall<'a>>
    where
        In: serde::Serialize,
        I: IntoIterator<Item = In>,
    {
        let inputs_json = inputs
            .into_iter()
            .map(|i| serde_json::to_string(&i).map_err(Error::Encode))
            .collect::<Result<Vec<_>>>()?;
        let function_call_id = self.app.remote_spawn_map(&self.name, inputs_json).await?;
        Ok(FunctionCall {
            app: self.app,
            function_call_id,
        })
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

/// A typed, positional CALL BUILDER produced by the `#[modal_rust::function]`
/// macro's generated `app.<fn>(args)` extension method (the auto-I/O ergonomics
/// path). Pure SUGAR over [`App::function`](crate::App::function) +
/// [`Function`]: it owns the macro-built named-input value (`In`, built from the
/// positional args) and pins the typed output (`Out`, the handler's return type),
/// so the caller chains into `.local()/.remote()/.spawn()/.map()` WITHOUT ever
/// naming a type.
///
/// The constructor takes `(&App, &'static str, In)`: the macro knows the
/// entrypoint `name` as a compile-time `&'static str` (the registry key) and the
/// already-built `In`, so the handle is construct-cheap and borrows the [`App`]
/// like [`Function`] does. Every method forwards verbatim to the matching
/// [`Function`] method, so the frozen serialize → registry/RUN path → decode
/// pipeline (and `retry_transient`/build-boundary/config, which live BELOW
/// `Function`) is unchanged.
pub struct TypedCall<'a, In, Out> {
    app: &'a crate::App,
    /// The frozen entrypoint key (registry name); a compile-time literal from the
    /// macro (the fn name, or the `name = "..."` override).
    name: &'static str,
    /// The generated named-input value, already built from the positional args.
    input: In,
    /// Pins the typed output without storing one.
    _out: std::marker::PhantomData<Out>,
}

impl<'a, In, Out> TypedCall<'a, In, Out>
where
    In: serde::Serialize,
    Out: serde::de::DeserializeOwned,
{
    /// Build a typed call over the entrypoint `name` with a pre-built `input`.
    /// Called by the macro-generated `app.<fn>(args)` method; rarely constructed
    /// by hand.
    pub fn new(app: &'a crate::App, name: &'static str, input: In) -> Self {
        TypedCall {
            app,
            name,
            input,
            _out: std::marker::PhantomData,
        }
    }

    /// Run IN-PROCESS via the frozen [`Registry`](crate::Registry), sugar over
    /// [`App::function(name).local`](Function::local). Returns the typed `Out`.
    pub fn local(self) -> Result<Out> {
        self.app.function(self.name).local::<In, Out>(self.input)
    }

    /// Run REMOTELY on Modal (the RUN path), sugar over
    /// [`App::function(name).remote`](Function::remote). Returns the typed `Out`.
    pub async fn remote(self) -> Result<Out> {
        self.app
            .function(self.name)
            .remote::<In, Out>(self.input)
            .await
    }

    /// Fire-and-forget spawn (the RUN path), sugar over
    /// [`App::function(name).spawn`](Function::spawn). Returns a typed
    /// [`TypedFunctionCall`] whose `.get().await?` decodes to `Out` without the
    /// caller naming a type.
    pub async fn spawn(self) -> Result<TypedFunctionCall<'a, Out>> {
        let call = self.app.function(self.name).spawn::<In>(self.input).await?;
        Ok(TypedFunctionCall {
            call,
            _out: std::marker::PhantomData,
        })
    }

    /// Fan-out over many inputs (the RUN path), sugar over
    /// [`App::function(name).map`](Function::map). The leading positional args only
    /// fixed the entrypoint + types; this handle's own `input` is DISCARDED and
    /// `map` runs the supplied iterator of `In` instead. Returns `Vec<Out>` in
    /// input order.
    pub async fn map<I>(self, inputs: I) -> Result<Vec<Out>>
    where
        I: IntoIterator<Item = In>,
    {
        self.app.function(self.name).map::<In, Out, I>(inputs).await
    }

    /// Tuple-unpacking fan-out, sugar over
    /// [`App::function(name).starmap`](Function::starmap). Like
    /// [`map`](TypedCall::map), this handle's own `input` is DISCARDED and the
    /// supplied iterator is run instead. Returns `Vec<Out>` in input order.
    pub async fn starmap<I>(self, inputs: I) -> Result<Vec<Out>>
    where
        I: IntoIterator<Item = In>,
    {
        self.app
            .function(self.name)
            .starmap::<In, Out, I>(inputs)
            .await
    }

    /// Side-effect fan-out (waits, discards outputs), sugar over
    /// [`App::function(name).for_each`](Function::for_each). This handle's own
    /// `input` is DISCARDED and the supplied iterator is run instead. Returns `()`.
    pub async fn for_each<I>(self, inputs: I) -> Result<()>
    where
        I: IntoIterator<Item = In>,
    {
        self.app.function(self.name).for_each::<In, I>(inputs).await
    }

    /// Fire-and-forget fan-out, sugar over
    /// [`App::function(name).spawn_map`](Function::spawn_map). This handle's own
    /// `input` is DISCARDED and the supplied iterator is run instead. Returns a
    /// [`FunctionCall`] for the map call without waiting for any output.
    pub async fn spawn_map<I>(self, inputs: I) -> Result<FunctionCall<'a>>
    where
        I: IntoIterator<Item = In>,
    {
        self.app
            .function(self.name)
            .spawn_map::<In, I>(inputs)
            .await
    }
}

/// A typed wrapper around [`FunctionCall`] returned by [`TypedCall::spawn`]: pins
/// the output type so `.get(timeout).await?` decodes to `Out` (= the handler's
/// return type) without the caller naming a type. Pure sugar over
/// [`FunctionCall::get`].
pub struct TypedFunctionCall<'a, Out> {
    call: FunctionCall<'a>,
    _out: std::marker::PhantomData<Out>,
}

impl<Out> TypedFunctionCall<'_, Out>
where
    Out: serde::de::DeserializeOwned,
{
    /// The spawned call's `function_call_id` (Modal's handle for the queued call).
    pub fn function_call_id(&self) -> &str {
        self.call.function_call_id()
    }

    /// Await the spawned call's result, sugar over [`FunctionCall::get`]. Decodes
    /// to the pinned `Out` type.
    pub async fn get(&self, timeout: Option<std::time::Duration>) -> Result<Out> {
        self.call.get::<Out>(timeout).await
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
