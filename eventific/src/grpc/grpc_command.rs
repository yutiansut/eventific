use std::fmt::Debug;
use crate::store::{Store};
use crate::Eventific;
use failure::Error;
use uuid::Uuid;
use slog::{Logger};
use futures::{Future, IntoFuture};
use crate::aggregate::Aggregate;
use grpc::{RequestOptions, SingleResponse, Metadata};
use prometheus::HistogramVec;
use prometheus::CounterVec;


lazy_static! {
    static ref HANDLE_COMMAND_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "eventific_handle_command_time_seconds",
        "The time it takes to handle a command",
        &[]
    )
    .unwrap();
    static ref HANDLE_COMMAND_ERROR_COUNTER: CounterVec = register_counter_vec!(
        "eventific_handle_command_error_count",
        "Number of error when handling a command",
        &["error"]
    )
    .unwrap();
}

pub fn grpc_command_new_aggregate<
    S: 'static + Default,
    D: 'static + Send + Sync + Debug + Clone + AsRef<str>,
    St: Store<D>,
    Input: 'static + Send,
    Resp: 'static + Send,
    IC: 'static + FnOnce(&Input) -> &str,
    VC: 'static + FnOnce(&Input) -> Result<Vec<D>, Error> + Send,
    RC: 'static + FnOnce() -> Resp + Send
>(
    eventific: &Eventific<S, D, St>,
    _ctx: RequestOptions,
    input: Input,
    id_callback: IC,
    event_callback: VC,
    result_callback: RC
) -> SingleResponse<Resp> {
    let timer = HANDLE_COMMAND_HISTOGRAM.with_label_values(&[]).start_timer();
    let logger = eventific.get_logger().clone();
    let err_logger = logger.clone();
    let eve = eventific.clone();

    let fut = get_uuid(&logger, &input, id_callback)
        .and_then(move |uuid| {
            event_callback(&input)
                .into_future()
                .map_err(move |_err| {
                    warn!(logger, "Validation failed");
                    grpc::Error::GrpcMessage(grpc::GrpcMessageError {
                        grpc_status: grpc::GrpcStatus::Argument as _,
                        grpc_message: "Validation failed".to_owned()
                    })
                })
                .and_then(move |events| {
                    eve.create_aggregate(uuid, events, None)
                        .map_err(move |err| {
                            warn!(err_logger, "Exception while creating aggregate"; "error" => format!("{}", err));
                            err.into()
                        })
                })
        })
        .and_then(move |_| {
            let res = result_callback();
            timer.observe_duration();
            Ok(res)
        })
        .map_err(|err| {
            HANDLE_COMMAND_ERROR_COUNTER.with_label_values(&[&err.to_string()]).inc();
            err
        });
    SingleResponse::metadata_and_future(Metadata::new(), fut)
}

pub fn grpc_command_existing_aggregate<
    S: 'static + Default + Send,
    D: 'static + Send + Sync + Debug + Clone + AsRef<str>,
    St: Store<D> + Sync,
    Input: 'static + Sync + Send + Clone,
    Resp: 'static + Send,
    IC: 'static + FnOnce(&Input) -> &str,
    VC: 'static + Fn(&Input, Aggregate<S>) -> IF + Send,
    RC: 'static + FnOnce() -> Resp + Send,
    IF: 'static + IntoFuture<Item=Vec<D>, Error=Error, Future=FF>,
    FF: 'static + Future<Item=Vec<D>, Error=Error> + Send
>(
    eventific: &Eventific<S, D, St>,
    _ctx: RequestOptions,
    input: Input,
    id_callback: IC,
    event_callback: VC,
    result_callback: RC
) -> SingleResponse<Resp> {
    let timer = HANDLE_COMMAND_HISTOGRAM.with_label_values(&[]).start_timer();
    let logger = eventific.get_logger().clone();
    let err_logger = logger.clone();
    let eve = eventific.clone();

    let fut = get_uuid(&logger, &input, id_callback)
        .and_then(move |uuid| {
            eve.add_events_to_aggregate(uuid, None, move |aggregate| {
                event_callback(&input, aggregate)
            })
                .map_err(move |err| {
                    warn!(err_logger, "Exception while creating aggregate"; "error" => format!("{}", err));
                    err.into()
                })
        })
        .and_then(move |_| {
            let res = result_callback();
            timer.observe_duration();
            Ok(res)
        })
        .map_err(|err| {
            HANDLE_COMMAND_ERROR_COUNTER.with_label_values(&[&err.to_string()]).inc();
            err
        });

    SingleResponse::metadata_and_future(Metadata::new(), fut)
}

fn get_uuid<Input, IC: 'static + FnOnce(&Input) -> &str>(logger: &Logger, input: &Input, id_callback: IC) -> impl Future<Item=Uuid, Error=grpc::Error> {
    let raw_id = id_callback(&input);
    let log = logger.new(o!("aggregate_id" => raw_id.to_owned()));
    Uuid::parse_str(raw_id)
        .into_future()
        .map_err(move |_err| {
            warn!(log, "Provided aggregate id was not a valid UUID");
            grpc::Error::GrpcMessage(grpc::GrpcMessageError {
                grpc_status: grpc::GrpcStatus::Argument as _,
                grpc_message: "Provided aggregate id was not a valid UUID".to_owned()
            })
        })
}
