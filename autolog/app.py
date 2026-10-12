"""GUI logic"""

import logging
import threading
import tkinter as tk
import webbrowser
from pathlib import Path
from tkinter import filedialog, messagebox, ttk
from typing import List, Optional, Tuple

import customtkinter as ctk

from autolog.__version__ import version
from autolog.constants import (
    APP_HEIGHT,
    APP_MIN_HEIGHT,
    APP_MIN_WIDTH,
    APP_WIDTH,
    BASE_TABLE_COLUMNS,
    DEFAULT_COLUMN_WIDTH,
    STATUS_DISPLAY,
    TABLE_COLUMN_WIDTHS,
    TRAILING_TABLE_COLUMNS,
)
from autolog.consumers.base import TargetField
from autolog.consumers.factory import get_consumer_class, get_consumers_names
from autolog.keyring_manager import CredentialManager
from autolog.logging_config import LOGGING_FILE
from autolog.models import ProcessingResult, WorklogEntry
from autolog.parsers.file_parsers import get_supported_formats
from autolog.update_helpers import get_latest_release_info, is_update_available
from autolog.widgets import (
    CellTooltip,
    CredentialsFrame,
    FileSelectorFrame,
    OptionsFrame,
    TargetsFrame,
    column_id,
)
from autolog.worklog_processor import WorklogProcessor

ctk.set_appearance_mode("System")
ctk.set_default_color_theme("blue")

logger = logging.getLogger(__name__)


class WorklogApp(ctk.CTk):
    """Main application window for the AutoLog tool."""

    def __init__(self):
        super().__init__()
        self.title("AutoLog")
        self.minsize(width=APP_MIN_WIDTH, height=APP_MIN_HEIGHT)
        self.geometry(f"{APP_WIDTH}x{APP_HEIGHT}")
        self.entries: List[WorklogEntry] = []
        self.editing_widget: Optional[tk.Widget] = None
        self._create_widgets()
        self.processor = WorklogProcessor(self.options_frame.selected_timezone)
        self._on_consumer_change(self.credentials_frame.consumer_name)
        self._setup_treeview()

        self.after(100, self._check_for_updates)

    @property
    def target_fields(self) -> Tuple[TargetField, ...]:
        return get_consumer_class(self.credentials_frame.consumer_name).target_fields

    def _create_widgets(self) -> None:
        """Initialize and arrange all UI components."""
        self.credentials_frame = CredentialsFrame(
            self, get_consumers_names(), self._on_consumer_change
        )
        self.credentials_frame.pack(pady=10, padx=10, fill="x")

        self.options_frame = OptionsFrame(self)
        self.options_frame.pack(pady=5, padx=10, fill="x")

        self.targets_frame = TargetsFrame(self, self._apply_target_to_all)

        self.file_frame = FileSelectorFrame(self, self._browse_file)
        self.file_frame.pack(pady=10, padx=10, fill="x")

        self._create_results_table()
        self._create_progress_controls()

    def _create_results_table(self) -> None:
        """Configure the treeview for displaying worklog results."""
        treestyle = ttk.Style()
        treestyle.theme_use("default")
        frame_bg_color = self._apply_appearance_mode(
            ctk.ThemeManager.theme["CTkFrame"]["fg_color"]
        )
        input_bg_color = self._apply_appearance_mode(
            ctk.ThemeManager.theme["CTkEntry"]["fg_color"]
        )
        text_color = self._apply_appearance_mode(
            ctk.ThemeManager.theme["CTkLabel"]["text_color"]
        )
        selected_color = self._apply_appearance_mode(
            ctk.ThemeManager.theme["CTkFrame"]["fg_color"]
        )

        treestyle.configure(
            "Treeview",
            background=frame_bg_color,
            foreground=text_color,
            fieldbackground=frame_bg_color,
            borderwidth=0,
        )
        treestyle.configure(
            "Treeview.Heading",
            background=input_bg_color,
            foreground=text_color,
            padding=(16, 8),
            borderwidth=0,
        )
        treestyle.map(
            "Treeview",
            foreground=[("selected", frame_bg_color)],
            background=[("selected", selected_color)],
        )
        treestyle.map("Treeview.Heading", background=[("active", selected_color)])

        self.tree = ttk.Treeview(self, show="headings")
        self.tree.pack(pady=10, padx=10, fill="both", expand=True)
        self.tooltip = CellTooltip(self.tree)

    def _configure_table_columns(self) -> None:
        """Rebuild table columns around the selected consumer's target fields."""
        columns = (
            *BASE_TABLE_COLUMNS,
            *(field.label for field in self.target_fields),
            *TRAILING_TABLE_COLUMNS,
        )
        self.tree.configure(columns=columns, displaycolumns=columns)
        for col in columns:
            self.tree.heading(col, text=col, anchor="center")
            self.tree.column(
                col,
                width=TABLE_COLUMN_WIDTHS.get(col, DEFAULT_COLUMN_WIDTH),
                anchor="center",
            )

    def _create_progress_controls(self) -> None:
        """Set up the progress bar and process button."""
        progress_frame = ctk.CTkFrame(self)
        progress_frame.pack(pady=5, padx=10, fill="x")
        self.status_label = ctk.CTkLabel(
            progress_frame, text="Ready", anchor="center", font=("Arial", 14)
        )
        self.status_label.pack(fill="x", expand=True)
        self.progress = ctk.CTkProgressBar(progress_frame, height=10)
        self.progress.set(0)
        self.progress.pack(pady=5, padx=10, fill="x")
        self.progress_color = self._apply_appearance_mode(
            ctk.ThemeManager.theme["CTkProgressBar"]["progress_color"]
        )
        self.process_btn = ctk.CTkButton(
            self, text="Process", command=self._start_processing, state="disabled"
        )
        self.process_btn.pack(pady=10)

    def _setup_treeview(self) -> None:
        """Bind events to the treeview."""
        self.tree.bind("<Double-1>", self._on_cell_double_click)

    def _on_consumer_change(self, consumer: str) -> None:
        """Swap credential fields, table columns and target pickers."""
        self._save_credentials()
        fields = get_consumer_class(consumer).credential_fields
        self.credentials_frame.set_fields(fields)
        self.credentials_frame.load_credentials(
            CredentialManager.get_credentials(
                consumer, tuple(field.key for field in fields)
            )
        )
        self._configure_table_columns()
        self._update_table()
        self.targets_frame.set_fields(self.target_fields, {})
        if self.target_fields:
            self.targets_frame.pack(pady=5, padx=10, fill="x", before=self.file_frame)
        else:
            self.targets_frame.pack_forget()
        self._resolve_targets()

    def _configure_consumer(self) -> None:
        self.processor.timezone = self.options_frame.selected_timezone
        self.processor.configure_consumer(
            self.credentials_frame.consumer_name,
            self.credentials_frame.credentials,
            self.options_frame.prevent_duplicates_var.get(),
        )

    def _save_credentials(self) -> None:
        credentials = self.credentials_frame.credentials
        if any(credentials.values()):
            CredentialManager.save_credentials(
                self.credentials_frame.consumer_name, credentials
            )

    def _browse_file(self) -> None:
        """Open a file dialog and load the selected file."""
        filetypes = [["Source File", get_supported_formats()]]
        file_path = filedialog.askopenfilename(filetypes=filetypes)
        if file_path:
            self.file_frame.set_file_path(file_path)
            self._load_entries(Path(file_path))

    def _load_entries(self, file_path: Path) -> None:
        """Load entries from the selected file."""
        self.processor.timezone = self.options_frame.selected_timezone
        try:
            provider_name = self.file_frame.provider_name
            self.entries, total_hours = self.processor.load_entries(
                file_path, provider_name
            )
        except Exception as e:
            self._update_status("Failed Loading Entries")
            self._show_error("Error", f"Failed Loading Entries\n{e}")
        else:
            if self.entries:
                self._update_status(f"Total: {total_hours}")
                self._update_table()
                self.process_btn.configure(state="normal")
                self._resolve_targets()
            else:
                self._update_table()
                self._update_status("Couldn't parse any entry")

    def _resolve_targets(self) -> None:
        """Ask the consumer to guess each entry's destination, in background."""
        if not self.target_fields or not self.entries:
            return
        if not all(self.credentials_frame.credentials.values()):
            self._update_status("Fill in credentials to match projects")
            return
        self._update_status("Matching projects and activities...")
        threading.Thread(target=self._resolve_targets_worker, daemon=True).start()

    def _resolve_targets_worker(self) -> None:
        try:
            self._configure_consumer()
            self.processor.resolve_targets(self.entries)
        except Exception as e:
            logger.exception("Target resolution failed")
            message = self._format_error(e)
            self.after(0, lambda: self._update_status("Could not load projects"))
            self.after(0, lambda: self._show_error("Error", message))
        else:
            self.after(0, self._on_targets_resolved)

    def _on_targets_resolved(self) -> None:
        consumer = self.processor.consumer
        choices = {
            field.label: sorted(
                {
                    choice
                    for entry in self.entries
                    for choice in consumer.choices(field, entry)
                }
            )
            for field in self.target_fields
        }
        self.targets_frame.set_fields(self.target_fields, choices)
        self._update_table()
        self._update_status(f"{len(self.entries)} entries ready")

    def _apply_target_to_all(self, field: TargetField, value: str) -> None:
        consumer = self.processor.consumer
        for entry in self.entries:
            consumer.retarget(entry, field, value)
        self._update_table()

    def _row_values(self, entry: WorklogEntry) -> tuple:
        duration = f"{entry.duration // 3600}h {(entry.duration % 3600) // 60}m"
        targets = tuple(
            getattr(entry, field.attr) or "⚠️ Missing" for field in self.target_fields
        )
        return (
            entry.started.strftime("%Y-%m-%d %H:%M"),
            duration,
            entry.issue_key or "⚠️ Missing",
            *targets,
            STATUS_DISPLAY.get(entry.status, STATUS_DISPLAY["pending"]),
            "",
        )

    def _update_table(self) -> None:
        """Refresh the treeview with current entries."""
        self.tree.delete(*self.tree.get_children())
        for entry in self.entries:
            self.tree.insert("", "end", values=self._row_values(entry))

    def _on_cell_double_click(self, event: tk.Event) -> None:
        """Handle double-click events for editing cells."""
        if self.tree.identify("region", event.x, event.y) != "cell":
            return
        col = self.tree.identify_column(event.x)
        row_id = self.tree.identify_row(event.y)
        if not row_id or self.tree.set(row_id, "Status") == STATUS_DISPLAY["success"]:
            return

        if col == column_id(self.tree, "Issue"):
            self._edit_issue_cell(row_id, col)
            return
        for field in self.target_fields:
            if col == column_id(self.tree, field.label):
                self._edit_target_cell(row_id, col, field)
                return

    def _entry_for_row(self, row_id: str) -> WorklogEntry:
        return self.entries[self.tree.index(row_id)]

    def _place_editor(self, widget: tk.Widget, row_id: str, column: str) -> None:
        x, y, width, height = self.tree.bbox(row_id, column)
        widget.place(x=x, y=y, width=width, height=height)
        widget.bind("<FocusOut>", lambda e: self._cancel_edit())
        widget.focus_set()
        self.editing_widget = widget

    def _edit_issue_cell(self, row_id: str, column: str) -> None:
        """Enable editing of an issue key cell."""
        entry = self._entry_for_row(row_id)
        editor = ctk.CTkEntry(self.tree)
        editor.insert(0, self.tree.set(row_id, "Issue"))
        editor.bind(
            "<Return>", lambda e: self._save_issue_edit(row_id, entry, editor.get())
        )
        self._place_editor(editor, row_id, column)

    def _edit_target_cell(self, row_id: str, column: str, field: TargetField) -> None:
        """Offer the destinations the consumer knows about for this entry."""
        entry = self._entry_for_row(row_id)
        editor = ttk.Combobox(
            self.tree,
            values=self.processor.consumer.choices(field, entry),
            state="readonly",
        )
        editor.set(getattr(entry, field.attr) or "")
        editor.bind(
            "<<ComboboxSelected>>",
            lambda e: self._save_target_edit(row_id, entry, field, editor.get()),
        )
        self._place_editor(editor, row_id, column)

    def _save_issue_edit(self, row_id: str, entry: WorklogEntry, value: str) -> None:
        entry.issue_key = value.strip()
        self._reset_entry_status(row_id, entry)

    def _save_target_edit(
        self, row_id: str, entry: WorklogEntry, field: TargetField, value: str
    ) -> None:
        self.processor.consumer.retarget(entry, field, value)
        self._reset_entry_status(row_id, entry)

    def _reset_entry_status(self, row_id: str, entry: WorklogEntry) -> None:
        entry.status = "pending"
        self.tree.item(row_id, values=self._row_values(entry))
        self._cancel_edit()

    def _cancel_edit(self) -> None:
        """Cancel the current edit operation."""
        if self.editing_widget:
            self.editing_widget.destroy()
            self.editing_widget = None

    def _start_processing(self) -> None:
        """Validate credentials and start processing in a background thread."""
        if not all(self.credentials_frame.credentials.values()):
            self._show_error("Error", "Please fill all credentials fields")
            return
        self._save_credentials()
        self._configure_consumer()
        self.process_btn.configure(state="disabled")
        self._update_progress(0, self.progress_color)
        self._update_status(f"Connecting to {self.credentials_frame.consumer_name}...")
        self._processing_thread = threading.Thread(
            target=self._process_entries, daemon=True
        )
        self._processing_thread.start()

    def _process_entries(self) -> None:
        """Execute processing and handle results."""
        try:

            def callback(
                idx: int, total: int, entry: WorklogEntry, result: ProcessingResult
            ) -> None:
                self._update_row_status(entry._idx, entry, result)
                self._update_progress(idx / total)
                self._update_status(f"{idx}/{total}")

            self.processor.process_entries(self.entries, callback)
            self._update_progress(color="green")
            self._update_status("Finished")
            self._show_results()

        except Exception as e:
            self._show_error("Error", self._format_error(e))
            self._update_progress(color="red")
        finally:
            self.process_btn.configure(state="normal")
            self._update_progress(0, self.progress_color)
            self._update_status("")

    def _update_row_status(
        self, idx: int, entry: WorklogEntry, result: ProcessingResult
    ) -> None:
        """Update a treeview row with processing results."""
        row = self.tree.get_children()[idx]
        self.tree.set(row, "Status", STATUS_DISPLAY[entry.status])
        self.tree.set(
            row, "Error", "" if result.success else self._format_error(result.error)
        )

    def _format_error(self, error: Exception) -> str:
        """Format an error message for display."""
        consumer = self.processor.consumer
        return consumer.format_error(error) if consumer else str(error)

    def _update_progress(self, value: float = None, color: str = None) -> None:
        """Update the progress bar."""
        if value is not None:
            self.progress.set(value)
        if color is not None:
            self.progress.configure(progress_color=color)

    def _update_status(self, message: str) -> None:
        """Update the status label."""
        self.status_label.configure(text=message)

    def _show_results(self) -> None:
        """Display a summary of processing results."""
        total = self.processor.total
        failed_count = len(self.processor.failed_entries)
        success_count = total - failed_count
        message = (
            f"Posted {success_count}/{total} worklogs\n"
            f"{failed_count} failed - double-click to edit cells"
            if self.processor.failed_entries
            else f"Successfully posted {success_count}/{total} worklogs"
        )
        messagebox.showinfo("Processing Complete", message)

    def _show_error(self, title, message, **options):
        detail = options.pop("detail", None)
        logfile_msg = f"Log file:\n{LOGGING_FILE}"

        if not detail:
            detail = logfile_msg
        else:
            detail += f"\n{logfile_msg}"

        messagebox.showerror(title, message, detail=detail, **options)

    def _check_for_updates(self) -> None:
        """
        In a background thread, fetch the latest release.
        If newer than version, prompt the user with Update / Ignore.
        """

        def _worker():
            try:
                info = get_latest_release_info("Bishrghalil/Autolog")
                if is_update_available(version, info["version"]):
                    # run prompt on the main thread
                    def _prompt():
                        msg = (
                            f"A new version {info['version']} is available.\n\n"
                            "Release notes:\n"
                            f"{info['changelog']}\n\n"
                            "View release page?"
                        )
                        if messagebox.askyesno(
                            "Update Available", msg, icon=messagebox.INFO
                        ):
                            webbrowser.open(info["url"])

                    self.after(0, _prompt)
            except Exception as e:
                logger.exception("Update check failed: %s", e)

        threading.Thread(target=_worker, daemon=True).start()
