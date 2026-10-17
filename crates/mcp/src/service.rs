//! The MCP protocol side of the server: initialize, tools/list, tools/call and the methods the
//! reference server does not know, with each call's work on its own thread behind the gate.

use std::borrow::Cow;
use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::Poll;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CompleteRequestParams, CompleteResult,
    ContentBlock, CustomRequest, CustomResult, ErrorCode, GetPromptRequestParams,
    GetPromptResponse, InitializeResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, SubscribeRequestParams,
    UnsubscribeRequestParams,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Map, Value as Json};
use tokio::sync::oneshot;

use crate::Server;
use crate::dispatch::{Answer, Call, answered, deadline, failed, prepared};
use crate::failure::Failure;
use crate::listing::{NEWEST, initialized};

// What the reference server answers to a method it does not know, and to one it knows and does
// not serve.
const UNKNOWN_METHOD: &str = "Invalid request parameters";
const NOT_FOUND: &str = "Method not found";

// The handler methods of the methods that the reference knows and does not serve: each answers
// the reference's method-not-found error.
macro_rules! not_served {
    ($($method:ident($params:ty) -> $answer:ty;)*) => {$(
        async fn $method(
            &self,
            _request: $params,
            _context: RequestContext<RoleServer>,
        ) -> Result<$answer, ErrorData> {
            Err(ErrorData::new(ErrorCode::METHOD_NOT_FOUND, NOT_FOUND, None))
        }
    )*};
}

impl ServerHandler for Server {
    not_served! {
        complete(CompleteRequestParams) -> CompleteResult;
        subscribe(SubscribeRequestParams) -> ();
        unsubscribe(UnsubscribeRequestParams) -> ();
    }

    fn get_info(&self) -> InitializeResult {
        initialized()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(&NEWEST))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.to_vec()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let answer = self
            .called(&request.name, request.arguments, context.ct.cancelled())
            .await;
        let blocks = vec![ContentBlock::text(answer.text)];
        let result = if answer.error {
            CallToolResult::error(blocks)
        } else {
            CallToolResult::success(blocks)
        };
        Ok(CallToolResponse::Complete(result))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let message = format!(
            "Resource not found: Unknown resource: {}",
            quoted(&request.uri)
        );
        Err(ErrorData::new(ErrorCode::RESOURCE_NOT_FOUND, message, None))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let message = format!("Unknown prompt: {}", quoted(&request.name));
        Err(ErrorData::new(ErrorCode(0), message, None))
    }

    async fn on_custom_request(
        &self,
        _request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        Err(unknown_method())
    }
}

fn unknown_method() -> ErrorData {
    ErrorData::new(
        ErrorCode::INVALID_PARAMS,
        UNKNOWN_METHOD,
        Some(Json::String(String::new())),
    )
}

impl Server {
    // One call: its checks, then its deadline, its permit and its thread. A cancellation stops
    // the deadline, so the thread ends at its next checkpoint and frees the permit then.
    async fn called(
        &self,
        name: &str,
        arguments: Option<Map<String, Json>>,
        cancelled: impl Future<Output = ()>,
    ) -> Answer {
        let call = match prepared(&self.inner, name, arguments) {
            Ok(call) => call,
            Err(refused) => return refused,
        };
        let deadline = deadline(&self.inner, &call).map(Arc::new);
        let mut cancelled = pin!(cancelled);
        let gate = Arc::clone(&self.gate).acquire_owned();
        let Some(permit) = first(gate, cancelled.as_mut()).await else {
            return self.cancelled(&call, deadline.as_deref());
        };
        let Ok(permit) = permit else {
            return self.unplanned(&call, "the concurrency gate is closed");
        };
        let (sender, receiver) = oneshot::channel();
        let inner = Arc::clone(&self.inner);
        let job_deadline = deadline.clone();
        let tool = call.tool.name;
        let job = Box::new(move || {
            let _permit = permit;
            drop(sender.send(answered(&inner, &call, job_deadline.as_deref())));
        });
        if let Err(error) = self.offload.start(job) {
            let detail = format!("the thread of the call did not start: {error}");
            return failed(&self.inner, tool, Failure::Unplanned(detail));
        }
        match first(receiver, cancelled.as_mut()).await {
            Some(Ok(answer)) => answer,
            Some(Err(_)) => {
                let detail = "the thread of the call ended without an answer".to_owned();
                failed(&self.inner, tool, Failure::Unplanned(detail))
            }
            None => {
                if let Some(deadline) = &deadline {
                    deadline.cancel();
                }
                failed(&self.inner, tool, Failure::Refused(cancelled_text()))
            }
        }
    }

    fn cancelled(&self, call: &Call, deadline: Option<&kernel::Deadline>) -> Answer {
        if let Some(deadline) = deadline {
            deadline.cancel();
        }
        failed(
            &self.inner,
            call.tool.name,
            Failure::Refused(cancelled_text()),
        )
    }

    fn unplanned(&self, call: &Call, detail: &str) -> Answer {
        failed(
            &self.inner,
            call.tool.name,
            Failure::Unplanned(detail.to_owned()),
        )
    }
}

// The output of `work` when it finishes first, or `None` when `cancelled` finishes first.
async fn first<T>(
    work: impl Future<Output = T>,
    mut cancelled: Pin<&mut impl Future<Output = ()>>,
) -> Option<T> {
    let mut work = pin!(work);
    poll_fn(|context| {
        if let Poll::Ready(value) = work.as_mut().poll(context) {
            return Poll::Ready(Some(value));
        }
        cancelled.as_mut().poll(context).map(|()| None)
    })
    .await
}

// The answer of a cancelled call, which the transport never sends.
fn cancelled_text() -> String {
    app::Error::from(kernel::Stopped::Cancelled).to_string()
}

// A name as the reference quotes it in its refusals.
fn quoted(name: &str) -> String {
    text::CodePointString::from(name).quoted()
}
