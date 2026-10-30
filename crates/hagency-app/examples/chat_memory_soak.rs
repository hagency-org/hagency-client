//! Real RoomScreen + SDK context windows on native D3D11, with no real account.
//! Build with --features chat-stress. Output/profile must be on D: on Windows.
#![recursion_limit = "256"]
use std::{
    cell::Cell,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};
use makepad_widgets::*;
use makepad_widgets::makepad_platform::event::finger::{ScrollEvent, ScrollPhase};
use matrix_sdk::{Client, config::SyncSettings};
use matrix_sdk_ui::timeline::{RoomExt, TimelineFocus, TimelineEventFocusThreadMode, TimelineItem};
use hagency_app::{
    home::room_screen::{RoomScreenWidgetRefExt, TimelineUpdate},
    render_maintenance::RenderMaintenance,
    sliding_sync,
    utils::RoomNameId,
    memory_budget,
};
use hagency_app::shared::image_viewer::ImageViewerWidgetRefExt;
use hagency_app::shared::file_upload_modal::FileUploadModalWidgetRefExt;
use serde_json::{json, Value};

#[cfg(not(feature = "heap-profile"))]
#[global_allocator]
static ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> = &stats_alloc::INSTRUMENTED_SYSTEM;
#[cfg(feature = "heap-profile")]
#[global_allocator]
static HEAP_ALLOCATOR: dhat::Alloc = dhat::Alloc;

app_main!(App);
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)) {
        ui: Root { main_window := Window {
            window.title: "Hagency isolated chat memory soak"
            window.inner_size: vec2(1100, 800)
            body +: { flow: Down
                status := Label {width: Fill height: 28 text: "Loading isolated SDK fixture"}
                room := RoomScreen {width: Fill height: Fill}
                viewer := Modal {content := ImageViewer {width: Fill height: Fill}}
                upload := Modal {content := FileUploadModal {width: Fill height: Fill}}
            }
        }}
    }
}
type Snapshot = (Client, imbl::Vector<Arc<TimelineItem>>, usize);
#[derive(Script, ScriptHook)]
struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    maintenance: RenderMaintenance,
    #[rust]
    timer: Timer,
    #[rust]
    ticks: usize,
    #[rust]
    contexts: usize,
    #[rust]
    started: Option<Instant>,
    #[rust]
    previous: Option<Instant>,
    #[rust]
    durations: Vec<f64>,
    #[rust]
    commands: Option<SyncSender<(usize, usize)>>,
    #[rust]
    snapshots: Option<Receiver<Snapshot>>,
    #[rust]
    updates: Option<hagency_app::timeline_updates::Sender>,
    #[rust]
    oldest_operation: Option<hagency_app::timeline_updates::Sender>,
    #[rust]
    output: Option<std::fs::File>,
    #[rust]
    png: Option<Arc<[u8]>>,
    #[cfg(feature = "heap-profile")]
    #[rust]
    profiler: Option<dhat::Profiler>,
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::theme_mod(vm);
        script_eval!(vm, {mod.theme = mod.themes.light});
        makepad_widgets::widgets_mod(vm);
        hagency_app::app::register_widgets(vm);
        self::script_mod(vm)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.maintenance.handle_event(cx, event);
        if matches!(event, Event::Startup) {
            let directory = PathBuf::from(
                std::env::var("HAGENCY_STRESS_OUTPUT").expect("isolated output directory"),
            );
            if cfg!(windows) {
                assert!(
                    directory.is_absolute(),
                    "HAGENCY_STRESS_OUTPUT must select an absolute output directory"
                );
            }
            std::fs::create_dir_all(&directory).unwrap();
            #[cfg(feature = "heap-profile")]
            {
                self.profiler = Some(
                    dhat::Profiler::builder()
                        .file_name(directory.join("heap.json"))
                        .build(),
                );
            }
            self.output = Some(std::fs::File::create(directory.join("samples.jsonl")).unwrap());
            let (commands, request) = mpsc::sync_channel(1);
            let (sender, snapshots) = mpsc::sync_channel(1);
            self.commands = Some(commands);
            self.snapshots = Some(snapshots);
            std::thread::spawn(move || run_sdk(request, sender));
            self.commands
                .as_ref()
                .unwrap()
                .send((500_000, 1_000_000))
                .unwrap();
            let mut png = std::io::Cursor::new(Vec::new());
            ::image::DynamicImage::ImageRgba8(::image::RgbaImage::from_fn(512, 512, |x, y| {
                ::image::Rgba([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8, 255])
            }))
            .write_to(&mut png, ::image::ImageFormat::Png)
            .unwrap();
            self.png = Some(png.into_inner().into());
            self.started = Some(Instant::now());
            self.timer = cx.start_interval(0.04);
        }
        if self.timer.is_event(event).is_some() {
            self.ticks += 1;
            if self.ticks == 2 && std::env::var_os("HAGENCY_STRESS_SHOW").is_some() {
                self.ui.window(cx, ids!(main_window)).restore(cx);
            }
            let now = Instant::now();
            if let Some(previous) = self.previous.replace(now) {
                if self.durations.len() == 4096 {
                    self.durations.remove(0);
                }
                self.durations
                    .push(now.duration_since(previous).as_secs_f64() * 1000.);
            }
            if let Ok((client, items, target)) = self.snapshots.as_ref().unwrap().try_recv() {
                let room_id = ruma::room_id!("!room:example.org").to_owned();
                let first = self.updates.is_none();
                self.updates = Some(sliding_sync::install_stress_endpoints(
                    client, room_id.clone(), self.updates.as_ref(),
                ));
                if first {
                    self.oldest_operation = self.updates.clone();
                    self.ui.room_screen(cx, ids!(room)).set_displayed_room(
                        cx,
                        &RoomNameId::new(matrix_sdk::RoomDisplayName::Named("Isolated history soak".into()), room_id),
                        None,
                    );
                } else {
                    self.ui.room_screen(cx, ids!(room)).borrow_mut().unwrap().stress_reconnect(cx);
                }
                // Keep the first window's producer alive for the whole run,
                // reproducing a slow transfer across every endpoint replacement.
                self.oldest_operation.as_ref().unwrap().send(
                    TimelineUpdate::AttachmentDownloadReset("mxc://example.org/long-transfer".into()),
                ).unwrap();
                let images = items
                    .iter()
                    .filter_map(|item| item.as_event()?.event_id())
                    .filter_map(|id| {
                        let index = id.as_str().strip_prefix("$e")?.parse::<usize>().ok()?;
                        (index.is_multiple_of(5)).then(|| {
                            (
                                format!("mxc://example.org/image{index}").into(),
                                self.png.as_ref().unwrap().clone(),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                self.ui
                    .room_screen(cx, ids!(room))
                    .borrow_mut()
                    .unwrap()
                    .stress_seed_media(&images);
                let len = items.len();
                self.updates
                    .as_ref()
                    .unwrap()
                    .send(TimelineUpdate::NewItems {
                        new_items: items,
                        changed_indices: 0..len,
                        is_append: false,
                        clear_cache: true,
                    })
                    .unwrap();
                self.contexts += 1;
                let list = self.ui.portal_list(cx, ids!(room.timeline.list));
                list.set_tail_range(false);
                list.set_first_id_and_scroll(35, 0.);
                list.redraw(cx);
                self.ui.label(cx, ids!(status)).set_text(
                    cx,
                    &format!(
                        "Real RoomScreen · target {target} · contexts {} · ticks {}",
                        self.contexts, self.ticks
                    ),
                );
                SignalToUI::set_ui_signal();
            }
            if self.updates.is_some() {
                if self.ticks % 250 == 10 {
                    self.ui.modal(cx, ids!(viewer)).open(cx);
                    self.ui
                        .image_viewer(cx, ids!(viewer.content))
                        .show_loaded(cx, self.png.as_ref().unwrap());
                }
                if self.ticks % 250 == 25 {
                    self.ui.image_viewer(cx, ids!(viewer.content)).reset(cx);
                    self.ui.modal(cx, ids!(viewer)).close(cx);
                }
                if self.ticks % 250 == 60 {
                    self.ui.file_upload_modal(cx, ids!(upload.content)).stress_image_preview(cx, self.png.clone());
                    self.ui.modal(cx, ids!(upload)).open(cx);
                }
                if self.ticks % 250 == 75 {
                    self.ui.file_upload_modal(cx, ids!(upload.content)).stress_image_preview(cx, None);
                    self.ui.modal(cx, ids!(upload)).close(cx);
                }
                if self.ticks.is_multiple_of(750) {
                    self.ui.window(cx, ids!(main_window)).resize(
                        cx,
                        if (self.ticks / 750).is_multiple_of(2) {
                            dvec2(1100., 800.)
                        } else {
                            dvec2(720., 640.)
                        },
                    );
                }
                let list = self.ui.portal_list(cx, ids!(room.timeline.list));
                let bounds = list.area().rect(cx);
                if bounds.size.y > 0. {
                    let scroll = Event::Scroll(ScrollEvent {
                        window_id: self.ui.window(cx, ids!(main_window)).window_id().unwrap(),
                        scroll: dvec2(
                            0.,
                            if (self.ticks / 45).is_multiple_of(2) {
                                110.
                            } else {
                                -110.
                            },
                        ),
                        abs: bounds.pos + bounds.size * 0.5,
                        modifiers: KeyModifiers::default(),
                        handled_x: Cell::new(false),
                        handled_y: Cell::new(false),
                        is_mouse: true,
                        time: cx.seconds_since_app_start(),
                        phase: ScrollPhase::None,
                    });
                    self.ui.handle_event(cx, &scroll, &mut Scope::empty());
                }
            }
            if self.ticks.is_multiple_of(125) {
                let elapsed = self.started.unwrap().elapsed().as_secs();
                let total = if elapsed < 900 { 1_000_000 } else { 10_000_000 };
                let target = (self.contexts * 2_654_435_761usize) % (total - 102) + 51;
                let _ = self.commands.as_ref().unwrap().try_send((target, total));
                let mut timings = self.durations.clone();
                timings.sort_by(f64::total_cmp);
                let state = self
                    .ui
                    .room_screen(cx, ids!(room))
                    .borrow()
                    .unwrap()
                    .stress_snapshot(cx);
                let (heap_bytes, heap_limit) = cx.with_vm(|vm| {
                    vm.bx.heap.reconcile_heap_bytes();
                    (
                        vm.bx.heap.accounted_heap_bytes(),
                        vm.bx.heap.max_heap_bytes(),
                    )
                });
                let font_bytes = cx
                    .get_global_ref::<std::rc::Rc<
                        std::cell::RefCell<makepad_widgets::makepad_draw::text::fonts::Fonts>,
                    >>()
                    .map(|fonts| fonts.borrow().memory_bytes());
                let live_drawlists = cx.draw_lists.id_iter().count();
                let (tree_nodes, tree_paths) = cx.widget_tree().retained_entry_counts();
                #[cfg(not(feature = "heap-profile"))]
                let allocator = ALLOCATOR.stats();
                #[cfg(feature = "heap-profile")]
                let allocator = stats_alloc::Stats::default();
                let instances = &cx.draw_lists.1.allocations;
                let sample = json!({"elapsed_s":elapsed,"ticks":self.ticks,"contexts":self.contexts,"logical_history":total,
                    "state":state,"texture_bytes":cx.texture_pool_bytes(),"texture_slots":cx.texture_pool_slot_count(),
                    "instance_bytes":instances.bytes(),"instance_records":instances.record_count(),"refusals":instances.refusals(),
                    "completed":cx.frame_completed_serial(),"submitted":cx.frame_submission_serial(),
                    "image_working_bytes":memory_budget::image_budget_bytes(cx),
                    "heap_bytes":heap_bytes,"heap_limit":heap_limit,"live_drawlists":live_drawlists,
                    "tree_nodes":tree_nodes,"tree_paths":tree_paths,
                    "rust_allocated":allocator.bytes_allocated,"rust_deallocated":allocator.bytes_deallocated,"rust_reallocated":allocator.bytes_reallocated,
                    "font_atlas_bytes":font_bytes.map(|bytes|bytes.atlas_bytes),"font_layout_bytes":font_bytes.map(|bytes|bytes.layout_cache_bytes),
                    "recording_bytes":cx.draw_lists.1.recordings.bytes(),"recording_limit":cx.draw_lists.1.recordings.limit(),
                    "timer_p99_ms":timings[(timings.len()*99/100).min(timings.len()-1)]});
                writeln!(self.output.as_mut().unwrap(), "{sample}").unwrap();
                self.output.as_mut().unwrap().flush().unwrap();
            }
            let seconds = std::env::var("HAGENCY_STRESS_SECONDS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1800);
            if self.started.unwrap().elapsed().as_secs() >= seconds {
                #[cfg(feature = "heap-profile")]
                {
                    self.profiler.take();
                }
                cx.quit();
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

fn event(index: usize) -> Value {
    let content = if index.is_multiple_of(5) {
        json!({"msgtype":"m.image","body":format!("Image {index}"),
        "url":format!("mxc://example.org/image{index}"),"info":{"mimetype":"image/png","w":512,"h":512,"size":16384}})
    } else {
        json!({"msgtype":"m.text","body":format!("Message {index} 中文 👩‍💻\nA real SDK event with several lines.\n{}", "Memory stays bounded. ".repeat(index%12)),
        "format":"org.matrix.custom.html","formatted_body":format!("<p>Message <b>{index}</b> 中文 👩‍💻 <a href='https://example.org/'>link</a></p><p>{}</p>","Bounded history working set. ".repeat(index%12))})
    };
    json!({"event_id":format!("$e{index}"),"sender":"@alice:example.org","origin_server_ts":1_700_000_000_000u64+index as u64*1000,
        "type":"m.room.message","content":content})
}

fn run_sdk(requests: Receiver<(usize, usize)>, snapshots: SyncSender<Snapshot>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 8192];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") && request.len() < 65536 {
                match socket.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buffer[..n]),
                }
            }
            let text = String::from_utf8_lossy(&request);
            let path = text.split_whitespace().nth(1).unwrap_or("");
            let response = if path.contains("/sync?") {
                json!({"next_batch":"s1","rooms":{"join":{"!room:example.org":{
                    "state":{"events":[{"type":"m.room.create","state_key":"","event_id":"$create","sender":"@alice:example.org","origin_server_ts":1,
                        "content":{"creator":"@alice:example.org","room_version":"10"}},
                        {"type":"m.room.member","state_key":"@alice:example.org","event_id":"$member","sender":"@alice:example.org","origin_server_ts":2,
                        "content":{"membership":"join","displayname":"Soak user"}}]},
                    "timeline":{"events":[event(999999)],"limited":false,"prev_batch":"older"}}}}})
            } else if let Some(id) = path.split("/context/").nth(1) {
                let target: usize = id
                    .split('?')
                    .next()
                    .unwrap()
                    .replace("%24", "$")
                    .trim_start_matches("$e")
                    .parse()
                    .unwrap();
                json!({"event":event(target),"events_before":(target.saturating_sub(50)..target).rev().map(event).collect::<Vec<_>>(),
                    "events_after":(target+1..target+51).map(event).collect::<Vec<_>>(),"state":[],"start":format!("b{target}"),"end":format!("f{target}")})
            } else {
                json!({"chunk":[]})
            };
            let body = response.to_string();
            let _ = write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let rt = memory_budget::bounded_async_runtime().unwrap();
    rt.block_on(async move {
        let client = Client::builder().http_client(matrix_sdk::reqwest::Client::builder().no_proxy().build().unwrap())
            .homeserver_url(url).server_versions([ruma::api::MatrixVersion::V1_0]).build().await.unwrap();
        client.matrix_auth().restore_session(serde_json::from_value(json!({"access_token":"isolated-fixture","user_id":"@alice:example.org","device_id":"SOAK"})).unwrap(),Default::default()).await.unwrap();
        client.event_cache().subscribe().unwrap();
        client.sync_once(SyncSettings::default().timeout(Duration::ZERO)).await.unwrap();
        let room = client.get_room(ruma::room_id!("!room:example.org")).unwrap();
        while let Ok((target,_total)) = requests.recv() {
            let timeline = room.timeline_builder().with_focus(TimelineFocus::Event {
                target:format!("$e{target}").try_into().unwrap(),num_context_events:50,
                thread_mode:TimelineEventFocusThreadMode::Automatic {hide_threaded_events:true},
            }).build().await.unwrap();
            let items = timeline.items().await;
            assert!(items.len() <= 205);
            // /context events_before is newest-first; the SDK exposes the final
            // window in chronological order. Validate fixture protocol semantics.
            let positions: Vec<usize> = items.iter()
                .filter_map(|item| item.as_event()?.event_id())
                .filter_map(|id| id.as_str().strip_prefix("$e")?.parse().ok())
                .collect();
            assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "SDK window must be chronological");
            if snapshots.send((client.clone(),items,target)).is_err() {break;}
        }
    });
}
