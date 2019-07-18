use crate::{prelude::*, primitive::{read_varlen_slice, SmpteTime}};

/// Represents a fully parsed track event, with delta time.
#[derive(Copy, Clone, Debug)]
pub struct Event<'a> {
    pub delta: u28,
    pub kind: EventKind<'a>,
}
impl<'a> Event<'a> {
    /// The received raw slice should last until the end of the track.
    /// This function will cut down the slice to the appropiate size.
    /// Also, the incoming slice will be advanced to the next event.
    pub fn read(
        raw: &mut &'a [u8],
        running_status: &mut Option<u8>,
    ) -> Result<(&'a [u8], Event<'a>)> {
        let delta = u28::read_u7(raw).context("failed to read event deltatime")?;
        let (raw, kind) =
            EventKind::read(raw, running_status).context("failed to parse event")?;
        Ok((raw, Event { delta, kind }))
    }
}

/// Represents the different kinds of events.
#[derive(Copy, Clone, Debug)]
pub enum EventKind<'a> {
    Midi { channel: u4, message: MidiMessage },
    SysEx(&'a [u8]),
    Escape(&'a [u8]),
    Meta(MetaMessage<'a>),
}
impl<'a> EventKind<'a> {
    pub fn read(
        raw: &mut &'a [u8],
        running_status: &mut Option<u8>,
    ) -> Result<(&'a [u8], EventKind<'a>)> {
        //Keep the beggining of the old slice
        let old_slice = *raw;
        //Read status
        let mut status = *raw.get(0).ok_or(err_msg("failed to read status"))?;
        if status < 0x80 {
            //Running status!
            status = running_status.ok_or(err_msg("event missing status with no running status active"))?;
        } else {
            //Set running status
            *running_status = Some(status);
            //Advance slice 1 byte to consume status. Note that because we already did `get()`, we
            //can use panicking index here
            *raw = &raw[1..];
        }
        //Delegate further parsing depending on status
        let kind = match status {
            0x80...0xEF => {
                let channel = u4::from(status.bit_range(0..4));
                EventKind::Midi {
                    channel,
                    message: MidiMessage::read(raw, status)
                        .context("failed to read midi message")?,
                }
            }
            0xF0 => {
                EventKind::SysEx(read_varlen_slice(raw).context("failed to read sysex event")?)
            }
            0xF7 => EventKind::Escape(
                read_varlen_slice(raw).context("failed to read escape event")?,
            ),
            0xFF => {
                EventKind::Meta(MetaMessage::read(raw).context("failed to read meta event")?)
            }
            _ => bail!("invalid event status"),
        };
        //Figure out raw slice out of new slice (prefix of old_slice which doesn't intersect raw)
        let len = raw.as_ptr() as usize - old_slice.as_ptr() as usize;
        let raw = &old_slice[0..len];
        Ok((raw, kind))
    }
}

/// Represents a MIDI message, not an event.
///
/// If reading a MIDI message from some stream, use `EventKind::read` instead and discard non-midi
/// events.
/// This way running status is easily handled.
#[derive(Copy, Clone, Debug)]
pub enum MidiMessage {
    /// Stop playing a note.
    /// Fields are MIDI key and velocity.
    NoteOff(u7, u7),
    /// Start playing a note.
    /// Fields are MIDI key and velocity.
    NoteOn(u7, u7),
    /// Modify the velocity of a note after it has been played.
    /// Fields are MIDI key and velocity.
    Aftertouch(u7, u7),
    /// Modify the value of a MIDI controller.
    /// Fields are the controller to modify and the value to set it to.
    Controller(u7, u7),
    /// Change the program (also known as instrument) for a channel.
    /// The field is the program to set it to.
    ProgramChange(u7),
    /// Change the note velocity of a whole channel at once, without starting new notes.
    ChannelAftertouch(u7),
    /// Set the pitch bend value.
    /// The field is the value to set it to.
    /// A value of `0x2000` indicates no bend.
    PitchBend(u14),
}
impl MidiMessage {
    /// Receives a slice pointing to midi args (not including status byte)
    /// Status byte is given separately to reuse running status
    fn read(raw: &mut &[u8], status: u8) -> Result<MidiMessage> {
        Ok(match status.bit_range(4..8) {
            0x8 => MidiMessage::NoteOff(u7::read(raw)?, u7::read(raw)?),
            0x9 => MidiMessage::NoteOn(u7::read(raw)?, u7::read(raw)?),
            0xA => MidiMessage::Aftertouch(u7::read(raw)?, u7::read(raw)?),
            0xB => MidiMessage::Controller(u7::read(raw)?, u7::read(raw)?),
            0xC => MidiMessage::ProgramChange(u7::read(raw)?),
            0xD => MidiMessage::ChannelAftertouch(u7::read(raw)?),
            0xE => MidiMessage::PitchBend(u14::read_u7(raw)?),
            _ => bail!("invalid midi message status"),
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub enum MetaMessage<'a> {
    /// For `Format::Sequential` MIDI file types, `TrackNumber` can be empty, and defaults to
    /// track index.
    TrackNumber(Option<u16>),
    Text(&'a [u8]),
    Copyright(&'a [u8]),
    TrackName(&'a [u8]),
    InstrumentName(&'a [u8]),
    Lyric(&'a [u8]),
    Marker(&'a [u8]),
    CuePoint(&'a [u8]),
    ProgramName(&'a [u8]),
    DeviceName(&'a [u8]),
    MidiChannel(u4),
    MidiPort(u7),
    /// Obligatory at track end.
    EndOfTrack,
    /// Amount of microseconds per beat (quarter note).
    ///
    /// Usually appears at the beggining of a track, before any midi events are sent, but there
    /// are no guarantees.
    Tempo(u24),
    SmpteOffset(SmpteTime),
    /// In order of the MIDI specification, numerator, denominator, midi clocks per click, 32nd
    /// notes per quarter
    TimeSignature(u8, u8, u8, u8),
    /// As in the MIDI specification, negative numbers indicate number of flats, positive number
    /// of sharps `false` indicates major, `true` indicates minor.
    KeySignature(i8, bool),
    SequencerSpecific(&'a [u8]),
}
impl<'a> MetaMessage<'a> {
    fn read(raw: &mut &'a [u8]) -> Result<MetaMessage<'a>> {
        let type_byte = u8::read(raw).context("failed to read meta message type")?;
        let mut data = read_varlen_slice(raw).context("failed to read meta message data")?;
        Ok(match type_byte {
            0x00 => MetaMessage::TrackNumber(match data.len() {
                0 => None,
                2 => Some(u16::read(&mut data)?),
                _ => bail!("invalid data len"),
            }),
            0x01 => MetaMessage::Text(data),
            0x02 => MetaMessage::Copyright(data),
            0x03 => MetaMessage::TrackName(data),
            0x04 => MetaMessage::InstrumentName(data),
            0x05 => MetaMessage::Lyric(data),
            0x06 => MetaMessage::Marker(data),
            0x07 => MetaMessage::CuePoint(data),
            0x08 => MetaMessage::ProgramName(data),
            0x09 => MetaMessage::DeviceName(data),
            0x20 => {
                ensure!(data.len() == 1, "invalid data len");
                MetaMessage::MidiChannel(u4::read(&mut data)?)
            }
            0x21 => {
                ensure!(data.len() == 1, "invalid data len");
                MetaMessage::MidiPort(u7::read(&mut data)?)
            }
            0x2F => {
                ensure!(data.len() == 0, "invalid data len");
                MetaMessage::EndOfTrack
            }
            0x51 => {
                ensure!(data.len() == 3, "invalid data len");
                MetaMessage::Tempo(u24::read(&mut data)?)
            }
            0x54 => {
                ensure!(data.len() == 5, "invalid data len");
                MetaMessage::SmpteOffset(
                    SmpteTime::read(&mut data).context("failed to read smpte time")?,
                )
            }
            0x58 => {
                ensure!(data.len() == 4, "invalid data len");
                MetaMessage::TimeSignature(
                    u8::read(&mut data)?,
                    u8::read(&mut data)?,
                    u8::read(&mut data)?,
                    u8::read(&mut data)?,
                )
            }
            0x59 => {
                MetaMessage::KeySignature(u8::read(&mut data)? as i8, u8::read(&mut data)? != 0)
            }
            0x7F => MetaMessage::SequencerSpecific(data),
            _ => bail!("invalid meta event type"),
        })
    }
}
