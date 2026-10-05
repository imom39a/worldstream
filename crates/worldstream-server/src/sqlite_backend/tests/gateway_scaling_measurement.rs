//! Opt-in subprocess measurement. No admission limit or production gate is changed.
#![allow(clippy::unwrap_used, clippy::panic, clippy::too_many_lines)]
use super::*;
use crate::{OperatorState, operator_router};
use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};
use tungstenite::{Message, client::IntoClientRequest};
use worldstream_runtime::EffectiveConfig;
static PROJECTION_RESETS_OBSERVED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

type Socket = tungstenite::WebSocket<TcpStream>;
fn send(socket: &mut Socket, kind: &str, body: Value) {
    socket
        .send(Message::Text(
            serde_json::to_string(&worldstream_protocol::VersionedEnvelope {
                protocol: "0.1".into(),
                message_type: kind.into(),
                message_id: crate::next_ulid().unwrap(),
                request_id: None,
                body,
            })
            .unwrap()
            .into(),
        ))
        .unwrap();
}
fn receive(socket: &mut Socket) -> Result<Value, tungstenite::Error> {
    loop {
        match socket.read()? {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text).unwrap();
                if value["type"] == "server.ping" {
                    send(socket, "client.pong", json!({}));
                    continue;
                }
                return Ok(value);
            }
            Message::Ping(_) => {}
            other => panic!("unexpected wire: {other:?}"),
        }
    }
}
fn connect(
    address: SocketAddr,
    bearer: &str,
    room: &str,
    member: &str,
) -> Result<Socket, tungstenite::Error> {
    let mut request = format!("ws://{address}/v1/stream")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {bearer}").parse().unwrap());
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        worldstream_protocol::WEBSOCKET_SUBPROTOCOL.parse().unwrap(),
    );
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let (mut socket, _) = tungstenite::client(request, stream).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(error) => error,
        _ => panic!("interrupted blocking handshake"),
    })?;
    send(
        &mut socket,
        "client.hello",
        json!({"client_name":"gateway-measurement","client_version":"1","mode":"participant","supported_protocols":["0.1"],"capabilities":worldstream_protocol::REQUIRED_CLIENT_CAPABILITIES}),
    );
    assert_eq!(receive(&mut socket)?["type"], "server.welcome");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut attached = loop {
        send(
            &mut socket,
            "room.attach",
            json!({"room_id":room,"member_id":member,"after_frame_seq":null}),
        );
        let reply = receive(&mut socket)?;
        if reply["type"] == "room.attached" {
            break reply;
        }
        assert_eq!(reply["type"], "error", "attach response {reply}");
        assert!(
            reply["body"]["retryable"].as_bool().unwrap_or(false),
            "attach response {reply}"
        );
        assert!(Instant::now() < deadline, "attach retry deadline {reply}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        attached["body"]["cursor"].is_null(),
        "Cursor must require explicit observation.ack"
    );
    if attached["body"]["sync"]["kind"] == "projection_reset" {
        assert_eq!(receive(&mut socket)?["type"], "projection.reset");
        PROJECTION_RESETS_OBSERVED.fetch_add(1, Ordering::Relaxed);
    }
    send(
        &mut socket,
        "room.sync_ack",
        json!({"room_id":room,"member_id":member,"through_frame_head":attached["body"]["frame_head"],"sync_token":attached["body"]["sync_token"].take()}),
    );
    loop {
        let reply = receive(&mut socket)?;
        if reply["type"] == "room.sync_acked" {
            break;
        }
        assert_eq!(reply["type"], "projection.reset");
    }
    Ok(socket)
}
fn ingress(
    address: SocketAddr,
    bearer: &str,
    room: &str,
    digest: &str,
    basis: usize,
) -> (u16, Value) {
    let body = serde_json::to_string(&worldstream_protocol::ExternalInputIngressRequestV1 {
        version: worldstream_protocol::EXTERNAL_INPUT_INGRESS_REQUEST_VERSION.into(),
        source_id: worldstream_core::EXTERNAL_INPUT_INGRESS_SOURCE_ID.into(),
        input_id: crate::next_ulid().unwrap().to_string(),
        input_type: worldstream_core::EXTERNAL_INPUT_INGRESS_TYPE.into(),
        based_on_room_seq: basis as u64,
        pack_digest: digest.into(),
        payload: json!({}),
        recorded_at: None,
    })
    .unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    write!(stream,"POST /v1/rooms/{room}/external-input HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}
fn summary(samples: &[u128]) -> Value {
    if samples.is_empty() {
        return json!({"samples":0,"status":"not_measured"});
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let q = |n: usize| sorted[(sorted.len() * n / 100).min(sorted.len() - 1)];
    json!({"samples":sorted.len(),"p50_ns":q(50),"p95_ns":q(95),"p99_ns":q(99),"max_ns":sorted.last(),"quantile":"sorted exact bounded samples; floor index"})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in gateway measurement subprocess; WORLDSTREAM_GATEWAY_MEASUREMENT_OUTPUT required"]
async fn measure_actual_gateway_scaling() {
    let count: usize = std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_SESSIONS")
        .unwrap()
        .parse()
        .unwrap();
    let updates: usize = std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_UPDATES")
        .unwrap_or("100".into())
        .parse()
        .unwrap();
    let topology = std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_TOPOLOGY").unwrap();
    let pattern =
        std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_PATTERN").unwrap_or("paced".into());
    let selected = std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_FORMAT").unwrap();
    let publication_mode = std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_PUBLICATION_MODE")
        .unwrap_or("shared".into());
    assert!(matches!(
        publication_mode.as_str(),
        "shared" | "independent"
    ));
    let output = std::env::var("WORLDSTREAM_GATEWAY_MEASUREMENT_OUTPUT").unwrap();
    let format = if selected == "v2" {
        worldstream_core::CanonicalHistoryFormat::V2
    } else {
        worldstream_core::CanonicalHistoryFormat::V1
    };
    let (_directory, file) = database_fixture();
    let store = SqliteRoomStore::open(file.path()).unwrap();
    let authority = AuthorityV1::new(Arc::new(store.clone()));
    let principal: worldstream_core::PrincipalId =
        crate::next_ulid().unwrap().as_str().parse().unwrap();
    let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
    authority
        .bootstrap(
            AuthorityBootstrapV1::new(
                crate::next_ulid().unwrap().as_str().parse().unwrap(),
                principal.clone(),
                PrincipalKindV1::Human,
                crate::next_ulid().unwrap().as_str().parse().unwrap(),
                bearer.token_hash(),
                None,
            )
            .unwrap(),
            "2026-08-15T12:00:00Z".parse().unwrap(),
        )
        .unwrap();
    let registry =
        Arc::new(worldstream_core::gateway_benchmark_registry_for_conformance().unwrap());
    let descriptor = registry.catalog_revisions().next().unwrap();
    let retained = registry.load_retained(&descriptor.revision_digest).unwrap();
    let descriptor = retained.descriptor().clone();
    let digest = descriptor.revision_digest.to_string();
    let backend = Arc::new(SqliteGatewayBackend::new(store, registry));
    backend
        .independent_live_pages_for_measurement
        .store(publication_mode == "independent", Ordering::Relaxed);
    let host = session(0xa9, crate::next_ulid().unwrap().as_str());
    let auth = backend.authenticate(&host).unwrap().into_presented();
    let cohort = if topology == "same-membership" {
        count.min(64)
    } else if topology == "aggregate" {
        32
    } else {
        1
    };
    let room_count = if topology == "same-membership" {
        1
    } else {
        count.div_ceil(cohort)
    };
    let mut rooms = Vec::new();
    let mut credentials = Vec::new();
    for index in 0..room_count {
        let member_principal = if index == 0 {
            principal.clone()
        } else {
            let p: worldstream_core::PrincipalId =
                crate::next_ulid().unwrap().as_str().parse().unwrap();
            authority
                .change(
                    &auth,
                    worldstream_core::AuthorityChangeV1::CreatePrincipal {
                        change_id: crate::next_ulid().unwrap().as_str().parse().unwrap(),
                        principal_id: p.clone(),
                        kind: PrincipalKindV1::Human,
                    },
                    SqliteGatewayBackend::checked_at().unwrap(),
                )
                .unwrap();
            p
        };
        let request = CreateRoomRequest {
            pack: PackReference {
                id: descriptor.pack_id.clone(),
                version: descriptor.explanatory_version.clone(),
                digest: digest.clone(),
            },
            configuration: json!({"state_bytes":1024,"maximum_counter":100000}),
            members: vec![CreateMember {
                principal_id: member_principal.to_string(),
                principal_kind: PrincipalKind::Human,
                role: Some("counter".into()),
                access_mode: AccessMode::Participant,
            }],
            idempotency_key: crate::next_ulid().unwrap().to_string(),
        };
        let created = match backend
            .create_room_attempt(&host, &request, format)
            .unwrap()
        {
            super::super::CreateAttempt::Response(r) => *r,
            super::super::CreateAttempt::Retry => panic!("unexpected creation retry"),
        };
        let credential = backend
            .issue_member_capability(
                &host,
                MemberCapabilityIssueRequest {
                    room_id: created.room_id.clone(),
                    member_id: created.member_ids[0].clone(),
                    principal_id: member_principal.to_string(),
                    scopes: vec![
                        CapabilityScopeV1::RoomAttach,
                        CapabilityScopeV1::RoomObserveMember,
                    ],
                    idempotency_key: crate::next_ulid().unwrap().to_string(),
                    expires_at: None,
                },
            )
            .unwrap();
        rooms.push((created.room_id, created.member_ids[0].clone()));
        credentials.push(credential.bearer);
    }
    let state = OperatorState::new(EffectiveConfig::default())
        .unwrap()
        .with_backend(backend.clone());
    let live = state.live_streams.clone();
    let limiter = state.rate_limiter.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            operator_router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let wire = BearerWireV1::from_bytes([0xa9; 32]).to_wire();
    tokio::task::spawn_blocking(move||{
        let began=Instant::now();let mut sockets=Vec::new();let mut admitted_per_room=vec![0;room_count];
        let mut setup_retries=0;
        'connections: for i in 0..count.min(if topology=="same-membership" {65}else{count}) {
            let group=if topology=="same-membership" {0}else{i/cohort};
            let mut attempts=0;
            loop {match connect(address,&credentials[group],&rooms[group].0,&rooms[group].1) {
                Ok(mut socket)=>{socket.get_mut().set_nonblocking(true).unwrap();sockets.push((socket,group,0usize));admitted_per_room[group]+=1;break;},
                Err(tungstenite::Error::Http(response)) if topology=="same-membership" && i==64=>{assert_eq!(response.status().as_u16(),429);break 'connections;},
                Err(error)=>{attempts+=1;setup_retries+=1;assert!(attempts<=5,"setup connection {i}: {error:?}");std::thread::sleep(Duration::from_millis(150));},
            }}
            if i%16==15 {println!("GATEWAY_SETUP admitted={} elapsed_seconds={:.3}",sockets.len(),began.elapsed().as_secs_f64());
                for (socket,_,_) in &mut sockets {loop {match socket.read() {
                    Ok(Message::Text(text))=>{let value:Value=serde_json::from_str(&text).unwrap();assert_eq!(value["type"],"server.ping");send(socket,"client.pong",json!({}));},
                    Err(tungstenite::Error::Io(error)) if error.kind()==std::io::ErrorKind::WouldBlock=>break,
                    other=>panic!("setup keepalive {other:?}"),
                }}}
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let registered:usize=rooms.iter().map(|(room,_)|live.snapshots_for_room(room).len()).sum();assert_eq!(registered,sockets.len());
        let admission_metrics=limiter.prometheus_text();
        let principal_rejections:usize=admission_metrics.lines().find(|line|line.contains("scope=\"principal_connection\"")).unwrap().split_whitespace().last().unwrap().parse().unwrap();
        if topology=="same-membership" && count>64 {assert_eq!(principal_rejections,1);}
        let admitted=sockets.len();assert_eq!(admitted,if topology=="same-membership" {count.min(64)}else{count});
        for(socket,_,_) in &mut sockets {socket.get_mut().set_nonblocking(true).unwrap();}
        let setup_projection_resets=PROJECTION_RESETS_OBSERVED.load(Ordering::Relaxed);
        worldstream_core::reset_retained_pack_reduce_invocations_for_conformance();
        let before_records=backend.live_payload_records_read.load(Ordering::Relaxed);let before_payload_bytes=backend.live_payload_bytes_read.load(Ordering::Relaxed);let before_reads=backend.live_payload_page_reads_for_test();let before_body=crate::SHARED_BODY_PREPARATIONS.load(Ordering::Relaxed);let before_body_bytes=crate::LIVE_BODY_BYTES.load(Ordering::Relaxed);let before_envelopes=crate::LIVE_ENVELOPE_ENCODINGS.load(Ordering::Relaxed);let before_wire=crate::LIVE_ENVELOPE_BYTES.load(Ordering::Relaxed);let before_reservations=crate::publication::PREPARATION_RESERVATIONS.load(Ordering::Relaxed);
        let mut room_updates=vec![0usize;room_count];let mut rate_rejections=0;let mut accepted=0;let mut received=0;let mut submit=Vec::new();let mut delivery=Vec::new();let mut first_delivery=Vec::new();let mut identities=HashSet::new();let mut frame_hashes=std::collections::BTreeMap::new();let mut socket_poll_calls=0usize;let mut socket_poll_time_ns=0u128;let mut socket_poll_max_ns=0u128;let mut next_idle_sweep=Instant::now();
        let batch=if pattern=="burst" {8}else{1};
        for begin_round in (1..=updates).step_by(batch) {
            let round=(begin_round+batch-1).min(updates);
            let first=Instant::now();let mut last=Instant::now();
            for sequence in begin_round..=round {
                let group=(sequence-1)%room_count;
                let basis=(sequence-1)/room_count;
                let started=Instant::now();
                let (status,response)=loop {let result=ingress(address,&wire,&rooms[group].0,&digest,basis);if result.0==429 {rate_rejections+=1;std::thread::sleep(Duration::from_millis(40));continue;}break result;};
                assert_eq!(status,200,"{response}");assert_eq!(response["duplicate"],false);submit.push(started.elapsed().as_nanos());accepted+=1;last=Instant::now();room_updates[group]+=1;
            }
            let mut first_seen=false;
            let deadline=Instant::now()+Duration::from_secs(30);
            while sockets.iter().enumerate().any(|(index,(_,group,seq))|!(pattern=="stalled-reader"&&index==0)&&*seq<room_updates[*group]) {
                if Instant::now()>=deadline {
                    let missing=sockets.iter().filter(|(_,group,seq)|*seq<room_updates[*group]).count();
                    let queues=live.queue_snapshots();
                    let diagnostic=json!({"status":"failed_delivery_deadline","round":round,"accepted_updates":accepted,"frames_received":received,"missing_recipients":missing,"socket_poll_calls":socket_poll_calls,"socket_poll_time_ns":socket_poll_time_ns,"socket_poll_max_ns":socket_poll_max_ns,"registered_sessions":rooms.iter().map(|(room,_)|live.snapshots_for_room(room).len()).sum::<usize>(),"payload_page_read_calls":backend.live_payload_page_reads_for_test()-before_reads,"successful_page_records_read":backend.live_payload_records_read.load(Ordering::Relaxed)-before_records,"body_encoding_count":crate::SHARED_BODY_PREPARATIONS.load(Ordering::Relaxed)-before_body,"envelope_encoding_count":crate::LIVE_ENVELOPE_ENCODINGS.load(Ordering::Relaxed)-before_envelopes,"queue_current_frames":queues[0].process_current,"queue_current_bytes":queues[1].process_current,"queue_backpressure_total":queues[0].backpressure_total,"preparation_reservations":crate::publication::PREPARATION_RESERVATIONS.load(Ordering::Relaxed)-before_reservations,"wall_seconds":began.elapsed().as_secs_f64()});
                    std::fs::write(&output,serde_json::to_vec_pretty(&diagnostic).unwrap()).unwrap();
                    panic!("delivery deadline diagnostic {diagnostic}");
                }let mut progress=false;let idle_sweep=Instant::now()>=next_idle_sweep;if idle_sweep {next_idle_sweep=Instant::now()+Duration::from_millis(100);}
                for(index,(socket,group,seq)) in sockets.iter_mut().enumerate() {if pattern=="stalled-reader"&&index==0 {continue;}if !idle_sweep && *seq>=room_updates[*group] {continue;}let poll_started=Instant::now();let poll_result=socket.read();let poll_ns=poll_started.elapsed().as_nanos();socket_poll_calls+=1;socket_poll_time_ns+=poll_ns;socket_poll_max_ns=socket_poll_max_ns.max(poll_ns);match poll_result{Ok(Message::Text(text))=>{let value:Value=serde_json::from_str(&text).unwrap();if value["type"]=="server.ping" {send(socket,"client.pong",json!({}));continue;}assert_eq!(value["type"],"observation.deliver");assert!(*seq<room_updates[*group],"unexpected Frame beyond accepted Room prefix");assert_eq!(value["body"]["room_id"],rooms[*group].0);assert_eq!(value["body"]["member_id"],rooms[*group].1);assert_eq!(value["body"]["frame_seq"],*seq+1);assert_eq!(value["body"]["cause_room_seq"],*seq+1);assert_eq!(value["body"]["observation"]["counter"],*seq+1);assert!(identities.insert(value["message_id"].as_str().unwrap().to_owned()));
                    let hash=value["body"]["frame_payload_hash"].as_str().unwrap().to_owned();
                    if let Some(old)=frame_hashes.insert((*group,*seq+1),hash.clone()) {assert_eq!(old,hash);}*seq+=1;received+=1;progress=true;if !first_seen {first_delivery.push(last.elapsed().as_nanos());first_seen=true;}},Err(tungstenite::Error::Io(e)) if e.kind()==std::io::ErrorKind::WouldBlock=>{},other=>panic!("receive {other:?}")}}
                if !progress {std::thread::sleep(Duration::from_millis(1));}
            }
            delivery.push(last.elapsed().as_nanos());assert!(first.elapsed()<Duration::from_secs(60));if round%10==0 || round==updates {println!("GATEWAY_UPDATES accepted={accepted} received={received} elapsed_seconds={:.3}",began.elapsed().as_secs_f64());}
        }
        if pattern=="stalled-reader" {
            let (socket,group,seq)=&mut sockets[0];socket.get_mut().set_nonblocking(false).unwrap();
            while *seq<room_updates[*group] {
                let value=receive(socket).unwrap();assert_eq!(value["type"],"observation.deliver");assert_eq!(value["body"]["frame_seq"],*seq+1);assert_eq!(value["body"]["cause_room_seq"],*seq+1);assert_eq!(value["body"]["observation"]["counter"],*seq+1);
                assert!(identities.insert(value["message_id"].as_str().unwrap().to_owned()));assert_eq!(frame_hashes[&(*group,*seq+1)],value["body"]["frame_payload_hash"].as_str().unwrap());*seq+=1;received+=1;
            }
        }
        assert_eq!(received,room_updates.iter().zip(&admitted_per_room).map(|(updates,sessions)|updates*sessions).sum::<usize>());
        for (_,group,seq) in &sockets {assert_eq!(*seq,room_updates[*group]);}
        for (group,(room,member)) in rooms.iter().enumerate() {
            let test_session=GatewaySession::new_with_wire(crate::next_ulid().unwrap(),CapabilityBearerV1::from_bytes(BearerWireV1::parse(&credentials[group]).unwrap().into_bytes()),BearerWireV1::parse(&credentials[group]).unwrap());
            let authenticated=backend.authenticate(&test_session).unwrap();
            let grant=backend.authority().authorize_member_read(&authenticated.into_presented(),room.parse().unwrap(),member.parse().unwrap(),worldstream_core::MemberReadOperationV1::CatchUp,SqliteGatewayBackend::checked_at().unwrap()).unwrap();
            let durable=backend.store.read_observation_suffix(grant,0).unwrap();
            assert_eq!(durable.len(),room_updates[group]);
            for frame in durable {assert_eq!(frame_hashes[&(group,frame.frame_seq() as usize)],frame.payload_hash().to_string());assert_eq!(frame.cause_room_seq().get(),frame.frame_seq());}
        }
        let callbacks:usize=rooms.iter().map(|(room,_)|backend.trace_cache.with_room(&room.parse().unwrap(),|slot|{let trace=slot.as_ref().unwrap().trace();assert!(trace.transitions().is_empty());trace.activity_callback_count()}).unwrap()).sum();
        let process_callbacks=worldstream_core::retained_pack_reduce_invocation_count_for_conformance();assert!(process_callbacks>=callbacks);assert!(process_callbacks>=accepted);
        let queues=live.queue_snapshots();let fields=|q:&crate::InternalQueueSnapshot|json!({"current":q.process_current,"high_water":q.process_high_water,"activity_total":q.activity_total,"completed_total":q.completion_total,"backpressure_total":q.backpressure_total});
        if updates>0 {
            let (mut old,group,_)=sockets.remove(0);let _=old.close(None);drop(old);std::thread::sleep(Duration::from_millis(150));
            let mut fresh=connect(address,&credentials[group],&rooms[group].0,&rooms[group].1).unwrap();let _=fresh.close(None);
        }
        let payload_hash_sequence = frame_hashes.values().map(String::as_str).collect::<Vec<_>>().join("\n");
        let payload_hash_sequence_digest = worldstream_core::Blake3DigestV1::hash(payload_hash_sequence.as_bytes()).to_string();
        let mut report=json!({"publication_mode":publication_mode,"delivered_payload_hash_sequence_digest":payload_hash_sequence_digest,"schema":"worldstream/gateway-measurement/v1","pack_revision_digest":digest,"format":selected,"pattern":pattern,"topology":topology,"requested_sessions":count,"admitted_sessions":admitted,"registered_sessions":registered,"setup_connection_retries":setup_retries,"principal_connection_rejections":principal_rejections,"rejected_at_session":if count>64&&topology=="same-membership" {Some(65)}else{None},"requested_excess_attempts_not_made":if topology=="same-membership" {count.saturating_sub(65)}else{0},"rooms":room_count,"memberships_per_room":1,"sessions_per_principal_cohort":cohort,"requested_total_updates":updates,"per_room_update_counts":room_updates,"accepted_updates":accepted,"rate_rejected_submit_attempts":rate_rejections,"frames_expected":room_updates.iter().zip(&admitted_per_room).map(|(updates,sessions)|updates*sessions).sum::<usize>(),"frames_received":received,"durable_frame_hashes_verified":true,"intentionally_stalled_readers":if pattern=="stalled-reader" {1}else{0},"cursor_unchanged_reconnect_sample":updates>0});
        let counters=json!({"socket_poll_strategy":"Pending recipients each pass; inactive healthy sockets every100ms for keepalive","socket_poll_calls":socket_poll_calls,"socket_poll_time_ns":socket_poll_time_ns,"socket_poll_max_ns":socket_poll_max_ns,"payload_page_read_calls":backend.live_payload_page_reads_for_test()-before_reads,"successful_page_records_read":backend.live_payload_records_read.load(Ordering::Relaxed)-before_records,"successful_page_canonical_payload_bytes_read":backend.live_payload_bytes_read.load(Ordering::Relaxed)-before_payload_bytes,"body_encoding_count":crate::SHARED_BODY_PREPARATIONS.load(Ordering::Relaxed)-before_body,"body_encoding_bytes":crate::LIVE_BODY_BYTES.load(Ordering::Relaxed)-before_body_bytes,"envelope_encoding_count":crate::LIVE_ENVELOPE_ENCODINGS.load(Ordering::Relaxed)-before_envelopes,"envelope_encoding_bytes":crate::LIVE_ENVELOPE_BYTES.load(Ordering::Relaxed)-before_wire,"preparation_reservations":crate::publication::PREPARATION_RESERVATIONS.load(Ordering::Relaxed)-before_reservations,"preparation_logical_high_water_bytes":crate::publication::PREPARATION_HIGH_WATER_BYTES.load(Ordering::Relaxed),"retained_executor_reduce_callbacks":callbacks,"pack_reduce_callbacks":process_callbacks,"setup_projection_resets_observed":setup_projection_resets,"workload_projection_resets_observed":0,"workload_wire_close_messages_observed":0,"wire_lifecycle_counter_scope":"Observed messages only; unexpected workload Reset/Close fails; final explicit teardown excluded","pack_reduce_callback_counter_scope":"Process-wide Core retained-Pack reducer invocations since reset after setup; both formats and recovery; direct golden authoring excluded","submit_to_accepted":summary(&submit),"last_accepted_to_all_delivery":summary(&delivery),"last_accepted_to_first_observed_delivery":summary(&first_delivery),"frames_queue":fields(&queues[0]),"bytes_queue":fields(&queues[1]),"wall_seconds":began.elapsed().as_secs_f64(),"stimulus_note":"Accepted external inputs increment the gateway benchmark counter; exact Pack Observe emits each authorized current counter.","not_measured":["private-hidden scenario","reattach/revoke races (separate gateway correctness tests)","worker concurrency high water","slow-reader saturation close (small frames may fit transport buffers)"]});
        report.as_object_mut().unwrap().extend(counters.as_object().unwrap().clone());
        std::fs::write(output,serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        for(socket,_,_) in &mut sockets {let _=socket.close(None);}
    }).await.unwrap();
    server.abort();
}
