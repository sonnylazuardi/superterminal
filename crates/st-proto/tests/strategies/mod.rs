//! Proptest strategies for every wire type.
//!
//! Shared by the round-trip and framing integration tests.

#![allow(dead_code)]

use proptest::collection::{btree_map, vec};
use proptest::option;
use proptest::prelude::*;
use st_proto::control::*;
use st_proto::data::*;
use st_proto::frame::*;
use st_proto::*;

pub fn style_idx() -> impl Strategy<Value = StyleIdx> {
    (0u16..4096).prop_map(StyleIdx::new)
}

pub fn cell_flags() -> impl Strategy<Value = CellFlags> {
    any::<u8>().prop_map(CellFlags::from_bits_truncate)
}

pub fn attrs() -> impl Strategy<Value = Attrs> {
    any::<u16>().prop_map(Attrs::from_bits_truncate)
}

pub fn modes() -> impl Strategy<Value = Modes> {
    any::<u16>().prop_map(Modes::from_bits_truncate)
}

pub fn color() -> impl Strategy<Value = Color> {
    prop_oneof![
        Just(Color::Default),
        any::<u8>().prop_map(Color::Indexed),
        (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::Rgb(r, g, b)),
    ]
}

pub fn style() -> impl Strategy<Value = Style> {
    (color(), color(), color(), attrs()).prop_map(|(fg, bg, underline_color, attrs)| Style {
        fg,
        bg,
        underline_color,
        attrs,
    })
}

pub fn packed_cell() -> impl Strategy<Value = PackedCell> {
    (any::<u32>(), style_idx(), cell_flags())
        .prop_map(|(codepoint, style_idx, flags)| PackedCell::new(codepoint, style_idx, flags))
}

pub fn row() -> impl Strategy<Value = Row> {
    (
        vec(packed_cell(), 0..24),
        vec(".{0,4}", 0..4),
        any::<bool>(),
    )
        .prop_map(|(cells, extras, wrapped)| Row {
            cells,
            extras,
            wrapped,
        })
}

pub fn cursor() -> impl Strategy<Value = Cursor> {
    (
        any::<u16>(),
        any::<u16>(),
        prop_oneof![
            Just(CursorShape::Block),
            Just(CursorShape::Underline),
            Just(CursorShape::Beam)
        ],
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(row, col, shape, visible, blink)| Cursor {
            row,
            col,
            shape,
            visible,
            blink,
        })
}

pub fn surface_id() -> impl Strategy<Value = SurfaceId> {
    any::<u32>().prop_map(SurfaceId::new)
}

pub fn seq() -> impl Strategy<Value = Seq> {
    any::<u64>().prop_map(Seq::new)
}

pub fn abs_line() -> impl Strategy<Value = AbsLine> {
    any::<u64>().prop_map(AbsLine::new)
}

pub fn selection() -> impl Strategy<Value = Selection> {
    (
        prop_oneof![
            Just(SelectionKind::Normal),
            Just(SelectionKind::Block),
            Just(SelectionKind::Lines)
        ],
        abs_line(),
        any::<u16>(),
        abs_line(),
        any::<u16>(),
    )
        .prop_map(|(kind, aline, acol, hline, hcol)| Selection {
            kind,
            anchor: AbsPoint {
                line: aline,
                col: acol,
            },
            head: AbsPoint {
                line: hline,
                col: hcol,
            },
        })
}

pub fn view_state() -> impl Strategy<Value = ViewState> {
    (any::<u32>(), option::of(selection())).prop_map(|(scroll_offset, selection)| ViewState {
        scroll_offset,
        selection,
    })
}

pub fn exit_status() -> impl Strategy<Value = ExitStatus> {
    (option::of(any::<i32>()), option::of(any::<i32>()))
        .prop_map(|(code, signal)| ExitStatus { code, signal })
}

pub fn hello() -> impl Strategy<Value = Hello> {
    (
        (any::<u8>(), any::<u8>()),
        prop_oneof![
            Just(ClientKind::Control),
            Just(ClientKind::Data),
            Just(ClientKind::Tool)
        ],
        ".{0,12}",
    )
        .prop_map(|((major, minor), client_kind, build_id)| Hello {
            proto_version: ProtoVersion::new(major, minor),
            client_kind,
            build_id,
        })
}

pub fn hello_ack() -> impl Strategy<Value = HelloAck> {
    (".{0,12}", any::<u64>(), any::<u32>()).prop_map(|(server_build_id, rev, pid)| HelloAck {
        proto_version: PROTO_VERSION,
        server_build_id,
        workspace_revision: rev,
        server_pid: pid,
    })
}

pub fn reject() -> impl Strategy<Value = Reject> {
    (
        prop_oneof![
            Just(RejectReason::MajorMismatch),
            Just(RejectReason::BadMagic),
            Just(RejectReason::LineTooLong),
            Just(RejectReason::FrameTooLarge),
            Just(RejectReason::NotHello),
            Just(RejectReason::ShuttingDown),
        ],
        ".{0,20}",
    )
        .prop_map(|(reason, message)| Reject {
            reason,
            message,
            server_version: PROTO_VERSION,
        })
}

pub fn snapshot() -> impl Strategy<Value = Snapshot> {
    (
        (surface_id(), seq(), any::<u16>(), any::<u16>()),
        vec(style(), 1..6),
        vec(row(), 0..6),
        (cursor(), modes(), ".{0,16}"),
        (abs_line(), any::<u64>(), view_state()),
        option::of(exit_status()),
    )
        .prop_map(
            |(
                (surface_id, seq, cols, rows),
                styles,
                grid,
                (cursor, modes, title),
                (history_base, history_len, view_state),
                exited,
            )| Snapshot {
                surface_id,
                seq,
                cols,
                rows,
                styles,
                grid,
                cursor,
                modes,
                title,
                history_base,
                history_len,
                view_state,
                exited,
            },
        )
}

pub fn delta() -> impl Strategy<Value = Delta> {
    (
        (surface_id(), seq(), seq()),
        (abs_line(), any::<u64>()),
        option::of((any::<u16>(), any::<u16>())),
        vec((style_idx(), style()), 0..4),
        vec((any::<u16>(), row()), 0..5),
        (cursor(), modes(), option::of(".{0,16}")),
    )
        .prop_map(
            |(
                (surface_id, seq, since_seq),
                (history_base, history_len),
                resized,
                new_styles,
                rows,
                (cursor, modes, title),
            )| Delta {
                surface_id,
                seq,
                since_seq,
                history_base,
                history_len,
                resized,
                new_styles,
                rows: rows
                    .into_iter()
                    .map(|(index, row)| DirtyRow { index, row })
                    .collect(),
                cursor,
                modes,
                title,
            },
        )
}

pub fn data_msg() -> impl Strategy<Value = DataMsg> {
    prop_oneof![
        hello().prop_map(DataMsg::Hello),
        hello_ack().prop_map(DataMsg::HelloAck),
        reject().prop_map(DataMsg::Reject),
        (
            surface_id(),
            prop_oneof![Just(AttachMode::Active), Just(AttachMode::Passive)],
            any::<bool>(),
            seq()
        )
            .prop_map(
                |(surface_id, mode, want_snapshot, known_seq)| DataMsg::Attach(Attach {
                    surface_id,
                    mode,
                    want_snapshot,
                    known_seq
                })
            ),
        surface_id().prop_map(|surface_id| DataMsg::Detach(Detach { surface_id })),
        (surface_id(), vec(any::<u8>(), 0..64))
            .prop_map(|(surface_id, bytes)| DataMsg::Input(Input { surface_id, bytes })),
        (surface_id(), any::<u16>(), any::<u16>()).prop_map(|(surface_id, cols, rows)| {
            DataMsg::Resize(Resize {
                surface_id,
                cols,
                rows,
            })
        }),
        (surface_id(), abs_line(), any::<u16>()).prop_map(|(surface_id, from_line, count)| {
            DataMsg::FetchHistory(FetchHistory {
                surface_id,
                from_line,
                count,
            })
        }),
        (surface_id(), seq()).prop_map(|(surface_id, seq)| DataMsg::Ack(Ack { surface_id, seq })),
        snapshot().prop_map(|s| DataMsg::Snapshot(Box::new(s))),
        delta().prop_map(|d| DataMsg::Delta(Box::new(d))),
        (surface_id(), abs_line(), abs_line(), vec(row(), 0..6)).prop_map(
            |(surface_id, from_line, history_base, rows)| DataMsg::History(Box::new(History {
                surface_id,
                from_line,
                history_base,
                rows
            }))
        ),
        (surface_id(), seq(), exit_status()).prop_map(|(surface_id, seq, status)| {
            DataMsg::SurfaceExited(SurfaceExited {
                surface_id,
                seq,
                status,
            })
        }),
        surface_id().prop_map(|surface_id| DataMsg::Bell(Bell { surface_id })),
        (
            surface_id(),
            prop_oneof![
                Just(DetachReason::Requested),
                Just(DetachReason::SurfaceDestroyed),
                Just(DetachReason::ServerShutdown)
            ]
        )
            .prop_map(|(surface_id, reason)| DataMsg::Detached(Detached { surface_id, reason })),
        (option::of(surface_id()), any::<u16>(), ".{0,20}").prop_map(
            |(surface_id, code, message)| DataMsg::DataError(DataError {
                surface_id,
                code,
                message
            })
        ),
    ]
}

// ---------------------------------------------------------------- control plane

pub fn session_id() -> impl Strategy<Value = SessionId> {
    any::<u32>().prop_map(SessionId::new)
}

pub fn tab_id() -> impl Strategy<Value = TabId> {
    any::<u32>().prop_map(TabId::new)
}

pub fn spawn_spec() -> impl Strategy<Value = SpawnSpec> {
    (
        option::of(vec(".{0,8}", 1..3)),
        option::of(".{0,16}"),
        option::of(btree_map("[A-Z_]{1,8}", ".{0,8}", 0..3)),
        option::of(vec("[A-Z_]{1,8}", 0..3)),
        any::<u16>(),
        any::<u16>(),
    )
        .prop_map(|(shell, cwd, env, env_allow, cols, rows)| SpawnSpec {
            shell,
            cwd,
            env,
            env_allow,
            cols,
            rows,
        })
}

pub fn session() -> impl Strategy<Value = Session> {
    (
        session_id(),
        ".{0,10}",
        option::of(tab_id()),
        vec((tab_id(), surface_id()), 0..4),
    )
        .prop_map(|(id, name, active_tab, tabs)| Session {
            id,
            name,
            active_tab,
            tabs: tabs
                .into_iter()
                .map(|(id, surface)| Tab { id, surface })
                .collect(),
        })
}

pub fn workspace() -> impl Strategy<Value = Workspace> {
    (any::<u64>(), session_id(), vec(session(), 0..3)).prop_map(
        |(revision, active_session, sessions)| Workspace {
            revision,
            active_session,
            sessions,
        },
    )
}

pub fn surface_state() -> impl Strategy<Value = SurfaceState> {
    prop_oneof![
        Just(SurfaceState::Running),
        (option::of(any::<i32>()), option::of("SIG[A-Z]{2,5}"))
            .prop_map(|(code, signal)| SurfaceState::Exited { code, signal }),
    ]
}

pub fn surface_meta() -> impl Strategy<Value = SurfaceMeta> {
    (
        surface_id(),
        ".{0,10}",
        option::of(".{0,10}"),
        option::of("/[a-z/]{0,10}"),
        any::<u16>(),
        any::<u16>(),
        any::<bool>(),
        surface_state(),
        view_state(),
    )
        .prop_map(
            |(id, title, user_title, cwd, cols, rows, has_foreground_child, state, view_state)| {
                SurfaceMeta {
                    id,
                    title,
                    user_title,
                    cwd,
                    cols,
                    rows,
                    has_foreground_child,
                    state,
                    view_state,
                }
            },
        )
}

pub fn req() -> impl Strategy<Value = Req> {
    let rev = || option::of(any::<u64>());
    prop_oneof![
        any::<u32>().prop_map(|id| Req::WorkspaceGet { id }),
        any::<u32>().prop_map(|id| Req::WorkspaceSubscribe { id }),
        (any::<u32>(), ".{0,10}", rev()).prop_map(|(id, name, if_revision)| Req::SessionCreate {
            id,
            name,
            if_revision
        }),
        (any::<u32>(), session_id(), ".{0,10}", rev()).prop_map(
            |(id, session, name, if_revision)| Req::SessionRename {
                id,
                session,
                name,
                if_revision
            }
        ),
        (any::<u32>(), session_id(), rev()).prop_map(|(id, session, if_revision)| {
            Req::SessionDelete {
                id,
                session,
                if_revision,
            }
        }),
        any::<u32>().prop_map(|id| Req::SessionList { id }),
        (any::<u32>(), session_id())
            .prop_map(|(id, session)| Req::SessionSetActive { id, session }),
        (
            any::<u32>(),
            session_id(),
            option::of(any::<u32>()),
            option::of(spawn_spec()),
            option::of(surface_id()),
            rev()
        )
            .prop_map(
                |(id, session, index, spawn, surface, if_revision)| Req::TabCreate {
                    id,
                    session,
                    index,
                    spawn,
                    surface,
                    if_revision
                }
            ),
        (any::<u32>(), tab_id(), rev()).prop_map(|(id, tab, if_revision)| Req::TabClose {
            id,
            tab,
            if_revision
        }),
        (any::<u32>(), tab_id(), any::<u32>(), rev()).prop_map(|(id, tab, index, if_revision)| {
            Req::TabReorder {
                id,
                tab,
                index,
                if_revision,
            }
        }),
        (
            any::<u32>(),
            tab_id(),
            session_id(),
            option::of(any::<u32>()),
            rev()
        )
            .prop_map(|(id, tab, to_session, index, if_revision)| Req::TabMove {
                id,
                tab,
                to_session,
                index,
                if_revision
            }),
        (any::<u32>(), tab_id()).prop_map(|(id, tab)| Req::TabSetActive { id, tab }),
        (any::<u32>(), spawn_spec()).prop_map(|(id, spawn)| Req::SurfaceCreate { id, spawn }),
        (
            any::<u32>(),
            surface_id(),
            option::of(prop_oneof![
                Just(KillSignal::Hup),
                Just(KillSignal::Term),
                Just(KillSignal::Kill)
            ])
        )
            .prop_map(|(id, surface, signal)| Req::SurfaceKill {
                id,
                surface,
                signal
            }),
        (any::<u32>(), surface_id(), option::of(".{0,10}")).prop_map(
            |(id, surface, user_title)| Req::SurfaceRename {
                id,
                surface,
                user_title
            }
        ),
        (
            any::<u32>(),
            surface_id(),
            option::of(any::<u32>()),
            option::of(option::of(selection()))
        )
            .prop_map(|(id, surface, scroll_offset, selection)| Req::ViewSet {
                id,
                surface,
                scroll_offset,
                selection
            }),
        any::<u32>().prop_map(|id| Req::ServerStatus { id }),
        (any::<u32>(), option::of(any::<bool>()))
            .prop_map(|(id, force)| Req::ServerShutdown { id, force }),
    ]
}

pub fn ev() -> impl Strategy<Value = Ev> {
    prop_oneof![
        (any::<u64>(), workspace(), vec(surface_meta(), 0..3)).prop_map(
            |(revision, workspace, surfaces)| Ev::Workspace {
                revision,
                workspace,
                surfaces
            }
        ),
        (
            surface_id(),
            option::of(any::<i32>()),
            option::of("SIG[A-Z]{2,5}")
        )
            .prop_map(|(surface, code, signal)| Ev::SurfaceExited {
                surface,
                code,
                signal
            }),
        ".{0,20}".prop_map(|reason| Ev::ServerShuttingDown { reason }),
    ]
}

pub fn error_body() -> impl Strategy<Value = ErrorBody> {
    (
        prop_oneof![
            Just(ErrorCode::BadRequest),
            Just(ErrorCode::NotFound),
            Just(ErrorCode::Conflict),
            Just(ErrorCode::SpawnFailed),
            Just(ErrorCode::Unsupported),
            Just(ErrorCode::ShuttingDown),
            Just(ErrorCode::Internal),
        ],
        ".{0,20}",
        option::of(any::<u64>()),
    )
        .prop_map(|(code, message, data)| ErrorBody {
            code,
            message,
            data: data.map(|d| serde_json::json!({ "revision": d })),
        })
}
