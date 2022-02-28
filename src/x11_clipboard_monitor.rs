use std::str;

use thiserror::Error;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

// inspired by https://www.uninformativ.de/blog/postings/2017-04-02/0/POSTING-en.html and https://docs.rs/x11-clipboard/0.5.3/src/x11_clipboard/lib.rs.html

#[derive(Error, Debug)]
enum X11ClipboardMonitorError {
	#[error("clipboard conversion has failed")]
	ConversionFailed,
	#[error("incr x extension is unsupported")]
	IncrUnsupported,
	#[error("the selection has lost it's owner")]
	SelectionOrphaned
}

pub struct X11ClipboardMonitor {
	connection: RustConnection,
	receiver_window: Window,
	atoms: Atoms
}

struct Atoms {
	clipboard: Atom,
	utf8_string: Atom,
	receiver_property: Atom,
	incr: Atom,
}

impl X11ClipboardMonitor {
	pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
		let (connection, screen_num) = x11rb::connect(None).unwrap();
		let screen = &connection.setup().roots[screen_num];
		let receiver_window = connection.generate_id()?;

		connection.create_window(
			COPY_DEPTH_FROM_PARENT,
			receiver_window,
			screen.root,
			0,
			0,
			1,
			1,
			0,
			WindowClass::INPUT_OUTPUT,
			0,
			&CreateWindowAux::new(),
		)?;

		let atoms = Atoms {
			clipboard: connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom,
			utf8_string: connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom,
			receiver_property: connection.intern_atom(false, b"CLIPBOARD_RECEIVER")?.reply()?.atom,
			incr: connection.intern_atom(false, b"INCR")?.reply()?.atom,
		};

		connection.xfixes_query_version(100, 0)?.reply()?;

		connection.xfixes_select_selection_input(screen.root, atoms.clipboard, 1_u8)?.check()?;

		connection.flush()?;

		Ok(Self {
			connection,
			receiver_window,
			atoms
		})
	}

	pub fn next_clipboard_string(&self) -> Result<String, Box<dyn std::error::Error>> {
		let clipboard_changed_event;

		loop {
			match self.connection.wait_for_event()? {
				Event::XfixesSelectionNotify(event) => {
					clipboard_changed_event = event;
					break
				},
				_ => (),
			};
		}

		self.connection.get_selection_owner(self.atoms.clipboard)?
			.reply()
			.map_err(|_| Box::new(X11ClipboardMonitorError::SelectionOrphaned))?;

		self.connection.convert_selection(
			self.receiver_window,
			clipboard_changed_event.selection,
			self.atoms.utf8_string,
			self.atoms.receiver_property,
			clipboard_changed_event.timestamp
		)?.check()?;

		self.connection.flush()?;

		loop {
			match self.connection.wait_for_event()? {
				Event::SelectionNotify(event) => {
					if event.property == AtomEnum::NONE.into() {
						return Err(Box::new(X11ClipboardMonitorError::ConversionFailed));
					}

					break
				},
				_ => (),
			};
		}

		let conversion_property = self.connection.get_property(
			false,
			self.receiver_window,
			self.atoms.receiver_property,
			AtomEnum::ANY,
			0,
			0
		)?.reply()?;

		// should be implemented if large clipboard data should also be retrievable
		if conversion_property.type_ == self.atoms.incr {
			return Err(Box::new(X11ClipboardMonitorError::IncrUnsupported));
		}

		let conversion_property_value = self.connection.get_property(
			false,
			self.receiver_window,
			self.atoms.receiver_property,
			AtomEnum::ANY,
			0,
			conversion_property.bytes_after,
		)?.reply()?.value;

		Ok(str::from_utf8(&conversion_property_value)?.into())
	}
}