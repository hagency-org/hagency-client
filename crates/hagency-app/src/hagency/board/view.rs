use makepad_widgets::*;
use super::{KanbanCard, ProjectBoard, markdown::BoardMarkdown, media::BoardInlineImage};

makepad_widgets::script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let Text = RinxLabel {width: Fill height: Fit draw_text +: {color: mod.widgets.RINX_INK text_style: theme.font_regular{font_size: mod.widgets.RINX_BODY_SIZE}}}
    let Small = Text {draw_text +: {color: mod.widgets.RINX_MUTED text_style: theme.font_regular{font_size: mod.widgets.RINX_META_SIZE}}}
    let Hint = Small {flow:Flow.Right{wrap:true}}
    let Input = RobrixTextInput {width: Fill height: 38 margin: 0 padding: Inset{left: 12 right: 12 top: 9 bottom: 9}}
    let Select = RobrixSettingsDropDown {width: Fill height: 38 margin: 0}
    let Action = RinxButton {height: 36 padding: Inset{left: 14 right: 14 top: 0 bottom: 0}}
    mod.widgets.BoardInlineImage = #(BoardInlineImage::register_widget(vm)) {
        width:Fill height:Fit flow:Down spacing:6 padding:8
        picture := Image {width:Fill height:200 fit:ImageFit.Smallest}
        unavailable := Small {width:Fill}
        caption := Small {width:Fill}
    }
    mod.widgets.BoardMarkdown = #(BoardMarkdown::register_widget(vm)) {
        width:Fill height:Fit flow:Down
        body := RinxHtml {width:Fill height:Fit padding:0
            boardimage := mod.widgets.BoardInlineImage {}
        }
    }
    let Format = RinxGhostButton {width:28 height:30 padding:0}
    let Composer = RoundedView {width:Fill height:Fit flow:Down spacing:10 padding:12
        draw_bg +: {color:mod.widgets.RINX_SURFACE border_color:mod.widgets.RINX_BORDER border_size:1.0 border_radius:10.0}
        View {width:Fill height:30 spacing:4 align:Align{y:0.5}
            edit := RinxGhostButton {height:30 text:#(crate::i18n::tr("Edit Markdown")) i18n_text:"Edit Markdown"}
            preview := RinxGhostButton {height:30 text:#(crate::i18n::tr("Preview")) i18n_text:"Preview"}
            View {width:Fill height:1}
        }
        tools := View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:2
            bold := Format {text:"B" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
            italic := Format {text:"I" draw_text.text_style:theme.font_italic{font_size:mod.widgets.RINX_BODY_SIZE}}
            heading := Format {text:"H2"}
            bullet := Format {text:"•"}
            ordered := Format {text:"1."}
            quote := Format {text:">"}
            code := Format {text:"`"}
            block := Format {text:"</>"}
            link := Format {text:"↗"}
        }
        media_tools := View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
            insert_image := RinxGhostButton {height:30 text:#(crate::i18n::tr("Insert image")) i18n_text:"Insert image"}
            paste_image := RinxGhostButton {height:30 text:#(crate::i18n::tr("Paste image")) i18n_text:"Paste image"}
        }
        source_wrap := View {width:Fill height:Fit
            source := Input {height:190 is_multiline:true empty_text:#(crate::i18n::tr("Write in Markdown…")) i18n_empty_text:"Write in Markdown…"
                draw_cursor +: {color:mod.widgets.RINX_ACCENT}
                draw_selection +: {color:mod.widgets.RINX_TEXT_SELECTION_BG color_hover:mod.widgets.RINX_TEXT_SELECTION_BG color_focus:mod.widgets.RINX_TEXT_SELECTION_BG color_down:mod.widgets.RINX_TEXT_SELECTION_BG}
            }
        }
        rendered := mod.widgets.BoardMarkdown {visible:false width:Fill height:Fit padding:2}
    }
    mod.widgets.KanbanCard = #(KanbanCard::register_widget(vm)) {
        width: Fill height: Fit margin: Inset{bottom: 10} padding: 14 flow: Down spacing: 10
        show_bg: true cursor: MouseCursor.Hand
        draw_bg +: {
            color: mod.widgets.RINX_SURFACE
            hover: instance(0.0)
            lifted: instance(0.0)
            border_radius: uniform(12.0) border_size: instance(1.0) border_color: instance(mod.widgets.RINX_BORDER)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, self.border_radius)
                sdf.fill_keep(self.color.mix(mod.widgets.RINX_HOVER, self.hover).mix(mod.widgets.RINX_PANEL_BG, self.lifted))
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }
        View {width: Fill height: Fit spacing: 6 align: Align{y: 0.5}
            number := Small {width: Fill}
            priority_badge := RoundedView {width: Fit height: Fit padding: Inset{left:7 right:7 top:3 bottom:3}
                draw_bg +: {color:mod.widgets.RINX_PANEL_BG border_radius:5.0}
                priority := Small {width: Fit}
            }
            drag_handle := View {width: 22 height: 26 align: Align{x: 1.0 y: 0.5} cursor: MouseCursor.Move
                grip := Small {width: Fit text: "⠿"}
            }
        }
        title := Text {max_lines: 3 text_overflow: Ellipsis draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_BODY_SIZE}}
        tags := Small {max_lines: 1 text_overflow: Ellipsis}
        View {width: Fill height: Fit spacing: 8 align: Align{y:0.5}
            assignee := Small {width: Fill max_lines: 1 text_overflow: Ellipsis}
            due := Small {width: Fit}
        }
    }
    let Column = RoundedView {width: 280 height: Fill padding: 12 flow: Down spacing: 12
        draw_bg +: {color: mod.widgets.RINX_PANEL_BG border_radius: 12.0 border_size:0.0 border_color:mod.widgets.RINX_BORDER}
        View {width: Fill height: 32 spacing: 8 align: Align{y: 0.5}
            column_grip := View {width:20 height:32 cursor:MouseCursor.Hand align:Align{x:0.5 y:0.5}
                Small {width:Fit text:"≡"}
            }
            marker := RoundedView {width: 8 height: 8 draw_bg +: {color: mod.widgets.RINX_MUTED border_radius: 4.0}}
            heading := Text {width: Fill max_lines:1 text_overflow:Ellipsis draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_BODY_SIZE}}
            count_badge := RoundedView {width:Fit height:Fit padding:Inset{left:8 right:8 top:3 bottom:3}
                draw_bg +: {color:mod.widgets.RINX_SURFACE border_radius:6.0}
                count := Small {width: Fit}
            }
            add := RinxGhostButton {width:28 height:28 padding:0 text:"+"}
        }
        cards := PortalList {width: Fill height: Fill capture_overload: false
            Card := mod.widgets.KanbanCard {margin: Inset{right:14 bottom:10}}
            Empty := RoundedView {width: Fill height: 104 margin:Inset{right:14} padding: 16 align:Align{x:0.5 y:0.5}
                draw_bg +: {color:mod.widgets.RINX_SURFACE border_color:mod.widgets.RINX_BORDER border_size:1.0 border_radius:10.0}
                Small {width:Fit text: #(crate::i18n::tr("No tasks here yet")) i18n_text: "No tasks here yet"}
            }
            More := View {width: Fill height: 48 margin: Inset{right:14}
                more := RinxGhostButton {width: Fill height:36 text: #(crate::i18n::tr("Load more tasks")) i18n_text: "Load more tasks"}
            }
            Filler := View {width: Fill height: 12}
        }
        quick_add := RinxGhostButton {width:Fill height:36 align:Align{x:0.0 y:0.5} text:#(crate::i18n::tr("+ Add a card")) i18n_text:"+ Add a card"}
        quick_form := View {visible:false width:Fill height:Fit flow:Down spacing:8
            quick_notice := Hint {visible:false}
            quick_title := Input {height:64 is_multiline:true submit_on_enter:true empty_text:#(crate::i18n::tr("Enter a title…")) i18n_empty_text:"Enter a title…"}
            View {width:Fill height:34 spacing:8
                quick_save := RinxPrimaryButton {height:34 text:#(crate::i18n::tr("Add card")) i18n_text:"Add card"}
                quick_cancel := RinxGhostButton {height:34 text:#(crate::i18n::tr("Cancel")) i18n_text:"Cancel"}
            }
        }
    }
    mod.widgets.ProjectBoard = #(ProjectBoard::register_widget(vm)) {
        skip_widget_tree_search: true
        draw_dialog_bg +: {color:#0008}
        notification_slide: 1.0
        animator: Animator {
            notifications: {
                default: @hide
                show: AnimatorState {redraw:true from:{all:Forward{duration:0.18}} apply:{notification_slide:0.0}}
                hide: AnimatorState {redraw:true from:{all:Snap} apply:{notification_slide:1.0}}
            }
        }
        width: Fill height: Fill flow: Down spacing: 8
        toolbar := View {width: Fill height: Fit flow: Down spacing: 6
            toolbar_row := View {width:Fill height:Fit flow:Right spacing:6 align:Align{y:0.5}
                board_navigation := View {visible:false width:Fit height:32 spacing:6 align:Align{y:0.5}
                    boards := Select {width:220 height:32 labels:[#(crate::i18n::tr("Main Board"))]}
                    toggle_board_creation := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_ADD}
                }
            toolbar_actions := View {width: Fill{min:270} height: Fit flow: Flow.Right{wrap: true} spacing: 6 align: Align{x:1.0 y: 0.5}
                default_badge := mod.widgets.RinxDefaultBadge {visible:false label +: {i18n_text:"default"}}
                set_default := RinxGhostButton {visible:false height:32 text:#(crate::i18n::tr("Set as default")) i18n_text:"Set as default"}
                filters_toggle := RinxIconButton {width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/filter.svg")}
                clear_filters := RinxGhostButton {height:32 visible:false text:#(crate::i18n::tr("Clear filters")) i18n_text:"Clear filters"}
                my_tasks := RinxGhostButton {height:32 text:#(crate::i18n::tr("My tasks")) i18n_text:"My tasks"}
                notifications_holder := View {width:Fit height:32 spacing:0 align:Align{y:0.5}
                    notifications_button := RinxIconButton {width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/bell.svg")}
                    notifications_badge := UnreadBadge {}
                }
                refresh := RinxIconButton {width:32 height:32 draw_icon.svg:crate_resource("self://resources/icons/article_refresh.svg")}
                manage_columns := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_SETTINGS}
                sync_status := Small {visible:false width:Fit}
            }
            }
            board_creation_controls := View {visible:false width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:6
                board_name := Input {width:200 height:32 empty_text:#(crate::i18n::tr("Board name")) i18n_empty_text:"Board name"}
                create_board := Action {height:32 text:#(crate::i18n::tr("New Board")) i18n_text:"New Board"}
            }
            filters_panel := RoundedView {visible:false width:Fill height:Fit padding:10 spacing:8 flow:Flow.Right{wrap:true} align:Align{y:0.5}
                draw_bg +: {color:mod.widgets.RINX_SURFACE border_color:mod.widgets.RINX_BORDER border_size:1 border_radius:8}
                search := Input {width:220 height:32 empty_text:#(crate::i18n::tr("Search tasks…")) i18n_empty_text:"Search tasks…"}
                filter_state := Select {width:150 height:32 labels:[#(crate::i18n::tr("All statuses"))]}
                filter_assignee := Select {width:160 height:32 labels:[#(crate::i18n::tr("All members"))]}
                filter_label := Input {width:140 height:32 empty_text:#(crate::i18n::tr("Filter by label")) i18n_empty_text:"Filter by label"}
                filter_label_choice := Select {width:160 height:32 labels:[#(crate::i18n::tr("Existing labels"))]}
                overdue := RinxCheckBox {text:#(crate::i18n::tr("Overdue")) i18n_text:"Overdue"}
                archived := RinxCheckBox {text:#(crate::i18n::tr("Archived tasks")) i18n_text:"Archived tasks"}
                apply_filters := Action {height:32 text:#(crate::i18n::tr("Apply")) i18n_text:"Apply"}
                filters_close := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_CLOSE}
            }
        }
        filter_summary := Hint {visible:false}
        created_feedback := View {visible:false width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:10 align:Align{y:0.5}
            created_message := Text {width:Fill max_lines:2 flow:Flow.Right{wrap:true}}
            open_created := RinxGhostButton {height:32 text:#(crate::i18n::tr("Open card")) i18n_text:"Open card"}
        }
        notice := Text {visible: false draw_text.color: mod.widgets.RINX_MUTED}
        retry := Action {visible: false text: #(crate::i18n::tr("Retry original save")) i18n_text: "Retry original save"}
        workspace := View {width: Fill height: Fill spacing: 14
            board_scroll := ScrollXView {width: Fill height: Fill flow: Right spacing: 12
                todo := Column {}
                in_progress := Column {}
                blocked := Column {}
                in_review := Column {}
                done := Column {}
                custom_0 := Column {visible:false}
                custom_1 := Column {visible:false}
                custom_2 := Column {visible:false}
                custom_3 := Column {visible:false}
                custom_4 := Column {visible:false}
                custom_5 := Column {visible:false}
                custom_6 := Column {visible:false}

            }
        }
        // Kept out of normal layout; the board draws it in a pass-wide modal overlay.
        inspector_host := View {visible:false
        notifications_panel := RoundedView {visible:false width:420 height:Fill flow:Down padding:20 spacing:12
            draw_bg +: {color:mod.widgets.RINX_SURFACE border_color:mod.widgets.RINX_BORDER border_size:1.0 border_radius:10.0}
            View {width:Fill height:Fit spacing:10 align:Align{y:0.5}
                Text {width:Fill flow:Flow.Right{wrap:true} text:#(crate::i18n::tr("Project notifications · assignments, comments and changes")) i18n_text:"Project notifications · assignments, comments and changes"}
                notifications_close := RinxIconButton {width:32 height:32 draw_icon.svg:ICON_CLOSE}
            }
            View {width:Fill height:32
                notifications_read := Action {text:#(crate::i18n::tr("Mark all read")) i18n_text:"Mark all read"}
            }
            notifications_list := PortalList {width:Fill height:Fill capture_overload:false
                Item := View {width:Fill height:76 flow:Down spacing:3 padding:Inset{right:16 bottom:8}
                    open := RinxGhostButton {width:Fill height:30 padding:0 align:Align{x:0.0 y:0.5}}
                    detail := Small {max_lines:2 text_overflow:Ellipsis}
                }
                Filler := View {width:Fill height:12}
                Empty := Small {width:Fill height:40 text:#(crate::i18n::tr("No notifications yet")) i18n_text:"No notifications yet"}
            }
            notifications_older := Action {visible:false text:#(crate::i18n::tr("Older notifications")) i18n_text:"Older notifications"}
        }
            column_settings := RoundedView {visible:false width:760 height:620 padding:20 flow:Down spacing:14
                draw_bg +: {color:mod.widgets.RINX_SURFACE border_radius:12.0 border_size:1.0 border_color:mod.widgets.RINX_BORDER}
                View {width:Fill height:40 spacing:10 align:Align{y:0.5}
                    Text {width:Fill text:#(crate::i18n::tr("Manage columns")) i18n_text:"Manage columns" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_SECTION_SIZE}}
                    save_columns := RinxPrimaryButton {height:36 text:#(crate::i18n::tr("Save changes")) i18n_text:"Save changes"}
                    cancel_columns := Action {text:#(crate::i18n::tr("Cancel")) i18n_text:"Cancel"}
                }
                Hint {text:#(crate::i18n::tr("Choose a column to edit. Changes apply together when saved.")) i18n_text:"Choose a column to edit. Changes apply together when saved."}
                columns_notice := Text {visible:false flow:Flow.Right{wrap:true}}
                columns_retry := Action {visible:false text:#(crate::i18n::tr("Retry original save")) i18n_text:"Retry original save"}
                rebase_columns := Action {visible:false text:#(crate::i18n::tr("Use latest board revision")) i18n_text:"Use latest board revision"}
                View {width:Fill height:Fill spacing:20
                    View {width:210 height:Fill flow:Down spacing:10
                        column_list := PortalList {width:Fill height:Fill capture_overload:false
                            Item := View {width:Fill height:64 padding:Inset{right:12 bottom:6} flow:Down spacing:2
                                choose_column := RinxGhostButton {width:Fill height:32 align:Align{x:0.0 y:0.5}}
                                column_summary := Hint {width:Fill max_lines:1 text_overflow:Ellipsis}
                            }
                        }
                        add_column := Action {width:Fill text:#(crate::i18n::tr("+ Add column")) i18n_text:"+ Add column"}
                    }
                    ScrollYView {width:Fill height:Fill flow:Down spacing:12
                        View {visible:false edit_column := Select {}}
                        Text {text:#(crate::i18n::tr("Column name")) i18n_text:"Column name"}
                        column_name := Input {}
                        View {width:Fill height:Fit flow:Flow.Right{wrap:true} spacing:8
                            column_left := Action {text:#(crate::i18n::tr("Move earlier")) i18n_text:"Move earlier"}
                            column_right := Action {text:#(crate::i18n::tr("Move later")) i18n_text:"Move later"}
                        }
                        column_done := RinxCheckBox {text:#(crate::i18n::tr("Tasks in this column are complete")) i18n_text:"Tasks in this column are complete"}
                        column_done_hint := Hint {text:#(crate::i18n::tr("Completed tasks are excluded from overdue results and count toward subtask progress.")) i18n_text:"Completed tasks are excluded from overdue results and count toward subtask progress."}
                        column_limit_enabled := RinxCheckBox {text:#(crate::i18n::tr("Limit the number of tasks in this column")) i18n_text:"Limit the number of tasks in this column"}
                        column_limit_form := View {visible:false width:Fill height:Fit flow:Down spacing:6
                            Text {text:#(crate::i18n::tr("Maximum tasks")) i18n_text:"Maximum tasks"}
                            column_wip := Input {width:130 empty_text:#(crate::i18n::tr("e.g. 3")) i18n_empty_text:"e.g. 3"}
                            column_limit_hint := Hint {text:#(crate::i18n::tr("Counts every member's unarchived tasks in this column. When full, new tasks and incoming moves are blocked.")) i18n_text:"Counts every member's unarchived tasks in this column. When full, new tasks and incoming moves are blocked."}
                        }
                        View {width:Fill height:1 margin:Inset{top:8 bottom:8} show_bg:true draw_bg.color:mod.widgets.RINX_BORDER}
                        Text {text:#(crate::i18n::tr("Archive column")) i18n_text:"Archive column"}
                        Hint {text:#(crate::i18n::tr("Hide this column and keep its cards. Select an archived column here to restore it.")) i18n_text:"Hide this column and keep its cards. Select an archived column here to restore it."}
                        archive_column := Action {text:#(crate::i18n::tr("Archive column")) i18n_text:"Archive column"}
                        Hint {text:#(crate::i18n::tr("Delete is available only for empty columns, including archived tasks.")) i18n_text:"Delete is available only for empty columns, including archived tasks."}
                        remove_column := RinxGhostButton {height:36 text:#(crate::i18n::tr("Delete empty column")) i18n_text:"Delete empty column"}
                    }
                }
            }

            inspector := RoundedView {visible: false width: 760 height: 700 padding: 18 flow: Down spacing: 12
                draw_bg +: {color: mod.widgets.RINX_SURFACE border_radius: 12.0 border_size: 1.0 border_color: mod.widgets.RINX_BORDER}
                View {width: Fill height: 40 spacing: 8 align: Align{y: 0.5}
                    detail_heading := Text {width: Fill max_lines:1 text_overflow:Ellipsis draw_text.text_style: theme.font_bold{font_size: mod.widgets.RINX_SECTION_SIZE}}
                    save := RinxPrimaryButton {width: 82 height: 38 text: #(crate::i18n::tr("Save")) i18n_text: "Save"}
                    archive := RinxButton {height:38 text:#(crate::i18n::tr("Archive")) i18n_text:"Archive"}
                    close_detail := RinxIconButton {icon_walk: Walk{width:16 height:16} draw_icon.svg: ICON_CLOSE}
                }
                dialog_notice := Text {visible:false}
                dialog_retry := Action {visible:false text:#(crate::i18n::tr("Retry original save")) i18n_text:"Retry original save"}
                reload_latest := RinxGhostButton {visible: false width: Fill height:36 text: #(crate::i18n::tr("Discard draft and load latest")) i18n_text: "Discard draft and load latest"}
                detail_tabs := RoundedView {width:Fill height:40 spacing:4 padding:3
                    draw_bg +: {color:mod.widgets.RINX_PANEL_BG border_radius:9.0}
                    detail_tab := RinxGhostButton {width:Fill height:34 padding:0 text:#(crate::i18n::tr("Task details")) i18n_text:"Task details"}
                    discussion_tab := RinxGhostButton {width:Fill height:34 padding:0 text:#(crate::i18n::tr("Discussion")) i18n_text:"Discussion"}
                }
                media_drop_hint := Small {visible:false width:Fill draw_text.color:mod.widgets.RINX_ACCENT}
                editor_scroll := ScrollYView {width: Fill height: Fill flow: Down spacing: 12 padding:Inset{right:12 top:2}
                    detail_notice := Small {}
                    details_body := View {width:Fill height:Fit flow:Down spacing:12
                    Text {text: #(crate::i18n::tr("Title")) i18n_text: "Title"}
                    title := Input {empty_text: #(crate::i18n::tr("What needs to be done?")) i18n_empty_text: "What needs to be done?"}
                    create_status := View {visible:false width:Fill height:Fit flow:Down spacing:5
                        Text {text:#(crate::i18n::tr("Status")) i18n_text:"Status"}
                        new_status := Select {labels:[#(crate::i18n::tr("To do")) #(crate::i18n::tr("In progress")) #(crate::i18n::tr("Blocked")) #(crate::i18n::tr("In review")) #(crate::i18n::tr("Done"))]}
                    }
                    task_status := View {visible:false width:Fill height:Fit flow:Down spacing:6
                        Text {text: #(crate::i18n::tr("Status")) i18n_text: "Status"}
                        move_state := Select {labels: [#(crate::i18n::tr("To do")) #(crate::i18n::tr("In progress")) #(crate::i18n::tr("Blocked")) #(crate::i18n::tr("In review")) #(crate::i18n::tr("Done"))]}
                        View {width: Fill height: Fit spacing: 8
                            move_top := Action {text: #(crate::i18n::tr("Move to top")) i18n_text: "Move to top"}
                            move_bottom := Action {text: #(crate::i18n::tr("Move to bottom")) i18n_text: "Move to bottom"}
                        }
                    }
                    parent_link := View {visible:false width:Fill height:Fit spacing:6
                        open_parent := Action {width:Fill}
                        unlink_parent := Action {text:#(crate::i18n::tr("Unlink")) i18n_text:"Unlink"}
                    }
                    View {width: Fill height: Fit flow: Down spacing: 5
                        Text {text: #(crate::i18n::tr("Priority")) i18n_text: "Priority"}
                        priority := Select {labels: [#(crate::i18n::tr("Low")) #(crate::i18n::tr("Normal")) #(crate::i18n::tr("High")) #(crate::i18n::tr("Urgent"))]}
                    }
                    View {width:Fill height:Fit align:Align{y:0.5}
                        Text {width:Fill text:#(crate::i18n::tr("Assignee")) i18n_text:"Assignee"}
                        assign_me := RinxGhostButton {height:28 text:#(crate::i18n::tr("Assign to me")) i18n_text:"Assign to me"}
                    }
                    assignee := Select {labels: [#(crate::i18n::tr("Unassigned"))]}
                    Text {text: #(crate::i18n::tr("Labels")) i18n_text: "Labels"}
                    labels := Input {empty_text:#(crate::i18n::tr("e.g. Design, Follow-up")) i18n_empty_text:"e.g. Design, Follow-up"}
                    label_feedback := Hint {}
                    label_choice := Select {labels:[#(crate::i18n::tr("Add existing label…"))]}
                    label_help := Hint {text:#(crate::i18n::tr("Separate labels with commas. Remove a name to remove its label. Up to 12 labels, 32 characters each.")) i18n_text:"Separate labels with commas. Remove a name to remove its label. Up to 12 labels, 32 characters each."}
                    Text {text: #(crate::i18n::tr("Due date")) i18n_text: "Due date"}
                    View {width:Fill height:38 spacing:6
                        due_date := Input {empty_text:#(crate::i18n::tr("No due date")) i18n_empty_text:"No due date"}
                        open_calendar := RinxGhostButton {width:38 height:38 padding:9 spacing:0 text:""
                            draw_icon +: {svg: crate_resource("self://resources/icons/calendar.svg") color:mod.widgets.RINX_ACCENT}
                            icon_walk:Walk{width:20 height:20}
                        }
                    }
                    calendar := RoundedView {visible:false width:Fill height:Fit padding:10 spacing:4 flow:Down
                        draw_bg +: {color:mod.widgets.RINX_PANEL_BG border_radius:10.0 border_color:mod.widgets.RINX_BORDER border_size:1.0}
                        View {width:Fill height:32 spacing:4 align:Align{y:0.5}
                            calendar_prev := RinxGhostButton {width:32 height:30 padding:0 text:"‹"}
                            calendar_month := Text {width:Fill align:Align{x:0.5 y:0.5}}
                            calendar_next := RinxGhostButton {width:32 height:30 padding:0 text:"›"}
                        }
                        View {width:Fill height:24 spacing:2
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("Mo")) i18n_text:"Mo"}
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("Tu")) i18n_text:"Tu"}
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("We")) i18n_text:"We"}
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("Th")) i18n_text:"Th"}
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("Fr")) i18n_text:"Fr"}
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("Sa")) i18n_text:"Sa"}
                            Small {width:Fill align:Align{x:0.5 y:0.5} text:#(crate::i18n::tr("Su")) i18n_text:"Su"}
                        }
                        View {width:Fill height:30 spacing:2
                            calendar_day_0 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_1 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_2 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_3 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_4 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_5 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_6 := RinxGhostButton {width:Fill height:30 padding:0}
                        }
                        View {width:Fill height:30 spacing:2
                            calendar_day_7 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_8 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_9 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_10 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_11 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_12 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_13 := RinxGhostButton {width:Fill height:30 padding:0}
                        }
                        View {width:Fill height:30 spacing:2
                            calendar_day_14 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_15 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_16 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_17 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_18 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_19 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_20 := RinxGhostButton {width:Fill height:30 padding:0}
                        }
                        View {width:Fill height:30 spacing:2
                            calendar_day_21 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_22 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_23 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_24 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_25 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_26 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_27 := RinxGhostButton {width:Fill height:30 padding:0}
                        }
                        View {width:Fill height:30 spacing:2
                            calendar_day_28 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_29 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_30 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_31 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_32 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_33 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_34 := RinxGhostButton {width:Fill height:30 padding:0}
                        }
                        View {width:Fill height:30 spacing:2
                            calendar_day_35 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_36 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_37 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_38 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_39 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_40 := RinxGhostButton {width:Fill height:30 padding:0}
                            calendar_day_41 := RinxGhostButton {width:Fill height:30 padding:0}
                        }
                        View {width:Fill height:32 spacing:4
                            calendar_today := RinxGhostButton {width:Fill height:30 text:#(crate::i18n::tr("Today")) i18n_text:"Today"}
                            calendar_clear := RinxGhostButton {width:Fill height:30 text:#(crate::i18n::tr("Clear date")) i18n_text:"Clear date"}
                            calendar_close := RinxGhostButton {width:Fill height:30 text:#(crate::i18n::tr("Close")) i18n_text:"Close"}
                        }
                    }
                    Text {text: #(crate::i18n::tr("Description")) i18n_text: "Description"}
                    description_editor := Composer {}
                    attachments_section := RoundedView {width:Fill height:Fit flow:Down spacing:10 padding:12
                        draw_bg +: {color:mod.widgets.RINX_SURFACE border_color:mod.widgets.RINX_BORDER border_size:1.0 border_radius:10.0}
                        View {width:Fill height:Fit align:Align{y:0.5}
                            Text {width:Fill text:#(crate::i18n::tr("Attachments")) i18n_text:"Attachments"}
                            upload_attachment := Action {text:#(crate::i18n::tr("Add file")) i18n_text:"Add file"}
                        }
                        Small {text:#(crate::i18n::tr("Drop files here · up to 2 MB each, 20 per task")) i18n_text:"Drop files here · up to 2 MB each, 20 per task"}
                        attachment_list := PortalList {width:Fill height:1 capture_overload:false
                            Filler := View {width:Fill height:12}
                            Item := View {width:Fill height:74 flow:Down spacing:4 padding:Inset{top:6 bottom:6}
                                filename := Text {width:Fill max_lines:1 text_overflow:Ellipsis}
                                View {width:Fill height:30 spacing:6 align:Align{y:0.5}
                                    file_size := Small {width:Fill}
                                    open := RinxGhostButton {height:30 text:#(crate::i18n::tr("Preview")) i18n_text:"Preview"}
                                    download := RinxGhostButton {height:30 text:#(crate::i18n::tr("Download")) i18n_text:"Download"}
                                    remove := RinxGhostButton {height:30 text:#(crate::i18n::tr("Delete")) i18n_text:"Delete"}
                                }
                            }
                        }
                        attachment_delete := RoundedView {visible:false width:Fill height:Fit flow:Down spacing:8 padding:10
                            draw_bg +: {color:mod.widgets.RINX_PANEL_BG border_color:mod.widgets.RINX_BORDER border_size:1.0 border_radius:8.0}
                            delete_filename := Text {width:Fill max_lines:2}
                            Small {text:#(crate::i18n::tr("Delete this attachment? Images in descriptions and comments will become unavailable.")) i18n_text:"Delete this attachment? Images in descriptions and comments will become unavailable."}
                            View {width:Fill height:Fit spacing:8
                                delete_cancel := RinxGhostButton {text:#(crate::i18n::tr("Cancel")) i18n_text:"Cancel"}
                                delete_confirm := Action {text:#(crate::i18n::tr("Delete attachment")) i18n_text:"Delete attachment"}
                            }
                        }
                        attachment_preview := Image {visible:false width:Fill height:220 fit:ImageFit.Smallest}
                    }
                    checklist_section := View {width:Fill height:Fit flow:Down spacing:10
                        View {width:Fill height:Fit align:Align{y:0.5}
                            Text {width:Fill text:#(crate::i18n::tr("Checklist")) i18n_text:"Checklist" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
                            checklist_progress := Small {width:Fit}
                        }
                        checklist_items := PortalList {width:Fill height:1 capture_overload:false
                            Item := View {width:Fill height:46 spacing:6 align:Align{y:0.5}
                                checked := RinxCheckBox {width:25 text:""}
                                item_title := Input {width:Fill height:38}
                                item_up := RinxGhostButton {width:26 height:30 padding:0 text:"↑"}
                                item_remove := RinxGhostButton {width:26 height:30 padding:0 text:"×"}
                            }
                        }
                        View {width:Fill height:Fit spacing:6
                            checklist_new := Input {width:Fill empty_text:#(crate::i18n::tr("Add a checklist item…")) i18n_empty_text:"Add a checklist item…"}
                            checklist_add := Action {text:"+"}
                        }
                        Small {text:#(crate::i18n::tr("Checklist changes sync when you save the task.")) i18n_text:"Checklist changes sync when you save the task."}
                    }
                    subtasks_section := View {visible:false width:Fill height:Fit flow:Down spacing:10
                        View {width:Fill height:Fit align:Align{y:0.5}
                            Text {width:Fill text:#(crate::i18n::tr("Subtasks")) i18n_text:"Subtasks" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
                            subtask_progress := Small {width:Fit}
                        }
                        subtask_items := PortalList {width:Fill height:1 capture_overload:false
                            Item := View {width:Fill height:62 padding:6 flow:Down spacing:3
                                open_child := RinxGhostButton {width:Fill height:30 padding:0}
                                child_status := Small {max_lines:1 text_overflow:Ellipsis}
                            }
                        }
                        add_subtask := RinxButton {width:Fill height:36 text:#(crate::i18n::tr("Add subtask")) i18n_text:"Add subtask"}
                    }
                    }
                    existing_actions := View {visible: false width: Fill height: Fit flow: Down spacing: 12
                        Hr {width: Fill height: 1}

                        Text {text: #(crate::i18n::tr("Comments")) i18n_text: "Comments" draw_text.text_style:theme.font_bold{font_size:mod.widgets.RINX_BODY_SIZE}}
                        comments := mod.widgets.BoardMarkdown {width:Fill height:Fit padding:0}
                        more_comments := RinxGhostButton {visible: false height:36 text: #(crate::i18n::tr("Load more comments")) i18n_text: "Load more comments"}
                        first_comments := RinxGhostButton {visible:false height:36 text:#(crate::i18n::tr("Back to first comments")) i18n_text:"Back to first comments"}
                        comment_draft_hint := Small {}
                        comment_editor := Composer {source_wrap +: {source +: {height:120}}}
                        add_comment := Action {text: #(crate::i18n::tr("Add comment")) i18n_text: "Add comment"}

                    }
                }
            }
        }
        drag_notice := Small {visible: false text: #(crate::i18n::tr("Drop on a column to move. Use the task menu for precise placement.")) i18n_text: "Drop on a column to move. Use the task menu for precise placement."}
        drop_marker := Tooltip {abs_pos:vec2(0,0) content +: {width:226 height:3 padding:0
            draw_bg +: {color:mod.widgets.RINX_ACCENT border_size:0.0 border_radius:1.5}
            tooltip_label +: {visible:false}
        }}
        drag_preview := Tooltip {abs_pos:vec2(0,0) content := RoundedShadowView {width:Fit height:Fit padding:0
            draw_bg +: {color:mod.widgets.RINX_SURFACE border_radius:8.0 shadow_radius:16.0 shadow_offset:vec2(0,4) shadow_color:#0003}
            tooltip_label := RinxLabel {visible:false}
            floating := mod.widgets.KanbanCard {width:226 margin:0 cursor:MouseCursor.Move
                draw_bg +: {border_color:mod.widgets.RINX_ACCENT border_size:2.0}
            }
        }}
    }
}
