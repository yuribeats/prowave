# Pro Wave control interface

For an AI coding agent (Claude Code) or a script running on the same Mac as Pro Wave.

## What it is

The running app listens on a Unix socket and takes requests as lines of JSON. Every request
runs inside the app, on the thread that draws its window, through the same functions the menus,
the mixer and the Tracks area call. So a call does what the click does: it can be undone with
Edit > Undo, it marks the project as changed, autosave covers it, and the audio engine follows
it on the next frame. Nothing here reaches the audio thread.

- **Socket:** `~/Library/Application Support/prowave-mac/control.sock`, readable and writable by
  the user only (the file has mode 0600 and its folder 0700). There is no network port.
- **Public edition: off by default.** Choose Help > Allow Control by Other Programs to turn it
  on (the choice is kept for the next start; starting the app with `PROWAVE_CONTROL=1` turns it
  on too). Any program running as you can then open and save projects, take screenshots of the
  window and quit the app. `PROWAVE_NO_CONTROL=1` always turns it off.
  `PROWAVE_CONTROL_SOCKET=/some/path` puts the socket somewhere else.
- **Client:** `prowavectl` (in `~/.local/bin`). It only talks to the socket; it never starts
  the app.
- **Claude Code:** `prowavectl mcp` is an MCP server that offers every method as a tool
  (`prowave_transport_play`, `prowave_state_get`, ..., and `prowave_call` for any method by
  name).

## Is the app running?

```sh
prowavectl call rpc.ping          # {"pong": true}; exit code 3 when the app is not running
prowavectl call rpc.version       # version, commit, protocol, number of methods
```

`prowavectl` exits with 0 when the call worked, 1 when the app answered with an error (the
error is printed on stderr), 2 for a mistake in the command line, 3 when the app is not
running, 4 when the answer did not come in time.

## Using it

```sh
prowavectl methods                          # every method, by group
prowavectl describe transport.locate        # one method: parameters, types, result
prowavectl call <method> '<json object>'    # call it; the result is printed as JSON
prowavectl state                            # the whole project (state.get)
prowavectl events                           # print what happens, one JSON line each
```

Parameters are always a JSON object of named values. `rpc.describe` returns the same list
this document has, from the running app.

### Naming things

- **Tracks:** by id (`"t3"`), by name (`"Vox"`), or by number in the track list from 1 (`2`).
  Ids come from `state.get` / `track.list`. `"master"` is the Stereo Out where a strip is
  asked for.
- **Regions:** by id (`"r12"`), or by name when only one region has that name.
- **Effects:** `"insert:1"` is the first built-in effect of the strip, `"plugin:17"` a plug-in
  (the number is in `state.get`), or the effect's name.
- **Parameters:** by key (`"volume"`, `"pan"`, `"mute"`, `"send:0"`, `"fx:0:3"`,
  `"plugin:1:42"`; `plugin.params` lists them) or by name.

Ids stay the same from call to call while the app runs: a track or region that is moved,
resized, renamed or put on another track keeps its id. They are worked out by matching the
project against what the last call saw (by name, place, length and track), so they can be
lost when that is not enough to tell: two regions that look alike changing at once, or a
region moved to another track and resized in the same step by hand. Such a thing comes back
with a new id. Ids are not saved in the project file; they start again when the app starts.

### Places and lengths

A place in the song can be given four ways; give one:

| Parameter | Meaning |
|---|---|
| `bar` | Bar number from 1, as the app shows it. A fraction is a part of the bar: `5.5` is halfway through bar 5. |
| `position` | `"bar.beat"`, `"bar.beat.division"` or `"bar.beat.division.tick"`, all from 1, as the app's display: `"5.3"` is beat 3 of bar 5. Divisions are sixteenths; a division has 240 ticks. |
| `seconds` | Seconds from the start of the song. |
| `beat` | Quarter notes from the start, from 0. |

A second place in the same call has a prefix: `end_bar`, `end_position`, `end_seconds`,
`end_beat`. Lengths are `length_bars` or `length_beats` (quarter notes). Note starts and
lengths inside a region are quarter notes from the region's start. Levels are in dB; pan goes
from -1 (left) to 1 (right).

## Methods

<!-- methods:start (made from crates/prowave-app/src/control_api/registry.rs; do not edit by hand) -->
130 methods in 28 groups. A parameter in **bold** is required.

### rpc

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `rpc.describe` | Every method with its parameters and result, and the error codes. | method (string): only this method | {protocol, app, version, commit, socket, methods: [{name, group, summary, changes_project, params: [{name, type, required, description, unit?, values?, min?, max?}], result}], errors: [{name, number, meaning}]} |
| `rpc.version` | The app's version and commit, and the protocol's version. | none | {app, version, commit, protocol, methods, socket, pid, headless, uptime_seconds} |
| `rpc.ping` | Answers at once: a check that the app is there and its interface thread is running. | none | {pong: true} |
| `rpc.stats` | How long the app took over the last requests, and who is connected. | none | {handling: {requests, measured, mean_ms, median_ms, p95_ms, max_ms}, clients, subscribers, window_visible} |
| `rpc.wait` | Answers after the app has drawn some frames: the time a change needs to reach the engine, or a plug-in to open. | frames (integer, 1 to 600): frames to wait for (default 2)<br>timeout_ms (number, milliseconds, 1 to 60000): answer after this long at the latest (default 5000) | {waited_ms} |

### state

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `state.get` | The whole project and the state of the app in one object. | regions (boolean): include each track's regions (default true)<br>notes (boolean): include each MIDI region's notes (default true; false keeps a big project's answer small) | {project: {name, path, dirty, can_undo, can_redo}, transport, key, tuning_hz, tempo_changes, signatures, markers, tracks: [track.get], stereo_out, selection, busy, status, dialog} |

### project

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `project.get` | The project's name, file and whether it has unsaved changes. | none | {name, path, dirty, can_undo, can_redo} |
| `project.new` | Replaces the open project with a new, empty one (File > New). | if_unsaved (string: error / save / discard): what to do when the open project has unsaved changes: error (the default: refuse), save (save it to its file first) or discard (lose them) | the new project as project.get gives it |
| `project.close` | Closes the project, which leaves a new, empty one (File > Close Project). | if_unsaved (string: error / save / discard): what to do when the open project has unsaved changes: error (the default: refuse), save (save it to its file first) or discard (lose them) | the empty project as project.get gives it |
| `project.open` | Opens a .prowave file in place of the open project. | **path** (string): the .prowave file: an absolute path, or one starting with ~/<br>if_unsaved (string: error / save / discard): what to do when the open project has unsaved changes: error (the default: refuse), save (save it to its file first) or discard (lose them) | project.get's fields, with missing_media (audio files that could not be read) and the dialog that lists them when there are any |
| `project.save` | Saves the project to its file (File > Save). | none | project.get's fields |
| `project.save_as` | Saves the project to a new file, which becomes its file (File > Save As). | **path** (string): the file to write: an absolute path, or one starting with ~/ (.prowave is added when missing)<br>overwrite (boolean): replace a file that is already there | project.get's fields |
| `project.save_copy` | Writes a copy of the project to another file; the project keeps its own file. | **path** (string): the file to write<br>overwrite (boolean): replace a file that is already there | {copy, project} |
| `project.revert` | Reloads the project from its file, losing every change since the last save. | **discard_changes** (boolean): must be true: the changes are lost | project.get's fields |
| `project.undo` | Undoes the last step (Edit > Undo). | none | {done, can_undo, can_redo}; done is false when there was nothing to undo |
| `project.redo` | Does the last undone step again (Edit > Redo). | none | {done, can_undo, can_redo} |

### transport

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `transport.get` | Whether the song plays or records, where the playhead is, the cycle, the tempo and the click. | none | {playing, recording, counting_in, playhead: {bar, position, beat, seconds}, tempo, signature, cycle: {on, start, end}, metronome, count_in} |
| `transport.play` | Starts playing, from the playhead or from a place. | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | the transport as transport.get gives it |
| `transport.stop` | Stops playing and recording. | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | the transport; a place moves the playhead there after stopping |
| `transport.record` | Starts recording, as the Record key does: audio on the armed audio tracks, else MIDI or audio on the selected track; with the count-in when it is on. | on (boolean): false stops the recording and leaves the song playing (default true)<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | the transport, with armed_tracks and records_on (the selected track) |
| `transport.locate` | Moves the playhead. | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | the transport |
| `transport.cycle` | Turns the cycle (loop) on or off and sets its range. | on (boolean): cycle on or off<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>end_bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>end_position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>end_seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>end_beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | the transport; the range runs from the start place to the end place (bar 1 to end_bar 9 loops bars 1 to 8) |
| `transport.tempo` | Reads or sets the tempo in effect at the playhead (or at a place), as dragging the tempo in the control bar does. | bpm (number, bpm, 5 to 990): the new tempo<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | {bpm, at, tempo_changes} |
| `transport.signature` | Reads the time signatures, or sets the signature from a bar on. | numerator (integer, 1 to 99): beats in a bar<br>denominator (integer, 1 to 64): the note that is one beat: 1, 2, 4, 8, 16, 32 or 64<br>from_bar (integer, 1 to 9999): the first bar with this signature (default 1: the song's own) | {signatures: [{bar, numerator, denominator}]} |
| `transport.metronome` | Reads or sets the click. | on (boolean): click on or off<br>volume_db (number, dB, -60 to 0): its level<br>accent (boolean): the first beat of each bar clicks higher | {on, volume_db, accent} |
| `transport.count_in` | Reads or sets the count-in before recording. | on (boolean): count-in on or off<br>bars (integer, 1 to 4): how many bars | {on, bars} |

### batch

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `batch` | Runs several calls in order as one Undo step. Stops at the first that fails. | **calls** (array): the calls: [{"method": "...", "params": {...}}, ...]<br>rollback (boolean): when a call fails, put the project back as it was before the batch (default false: what the earlier calls did stays) | {results: [each call's result], count}; on a failure the error has failed_index (0-based), failed_method, results so far and rolled_back |

### track

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `track.list` | The tracks in order, without their regions. | none | {tracks: [track.get without regions]} |
| `track.get` | One track: its strip settings, instrument, effects, sends, automation lanes and regions. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>notes (boolean): include the notes of its MIDI regions (default true) | {id, number, name, kind, selected, mute, solo, arm, monitor, volume_db, pan, color, output, input?, instrument?, effects: [{ref, id, name, format, bypass, values?}], sends, indent, vca?, group?, automation: [{param, points}], regions: [region.get]} |
| `track.add` | Adds a track at the end of the list and selects it (Track > New ... Track). | kind (string: instrument / audio / drummer / aux / vca / external_midi): the kind of track (default instrument)<br>name (string): its name (default: the app's, such as "Inst 2")<br>instrument (string): for an instrument track: the instrument to load, an id or name from plugin.list | the new track as track.get gives it |
| `track.remove` | Removes a track with its regions (Track > Delete Track). A folder is released instead: its tracks stay. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1) | {removed, name, tracks} |
| `track.rename` | Renames a track. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>**name** (string): the new name | {id, name} |
| `track.move` | Puts a track at another place in the list. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>**to** (integer, 1 to 1024): its new number in the list, from 1 | {id, number, order: [ids]}; the app has no gesture for this, so it is done here with the project's own renumbering; folder and VCA tracks cannot be moved |
| `track.set` | Sets a track's strip: mute, solo, record arm, monitoring, level, pan, input, output, VCA, name. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>mute (boolean): mute (a folder mutes its tracks)<br>solo (boolean): solo<br>arm (boolean): record arm (R)<br>monitor (boolean): input monitoring (I), audio tracks<br>volume_db (number, dB, -60 to 6): the fader<br>pan (number, -1 to 1): the pan knob, -1 left to 1 right<br>input (string): audio tracks: "mono:N" or "stereo:N", N the first input channel from 1<br>output (string): "stereo_out" or "bus:1" to "bus:8"<br>vca (any): the VCA track that moves this track's level (id, name or number), or "none"<br>name (string): a new name | the track as track.list gives it |
| `track.send` | Adds, changes or removes a send from a track to a bus. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>**bus** (integer, 1 to 8): the bus<br>level (number, 0 to 1): the send knob, 0 to 1<br>pre_fader (boolean): send before the fader<br>remove (boolean): take the send away | the track as track.list gives it; a bus gets its Aux strip when it has none |

### plugin

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `plugin.list` | The instruments and effects that can be loaded: the app's own, and the Audio Units and VST3s on this Mac. Hidden ones (the settings file plugin-hidden; Apple's Audio Units unless plugin-hide-apple holds 0) are left out. | kind (string: instrument / effect): only instruments or only effects<br>search (string): only those whose name, maker, id or emulated hardware (emulates) contains this text | {count, plugins: [{id, name, maker, kind, format: built-in\|AU\|VST3, vst3_id?, turned_off_after_crash?, rating?, type?, emulates?, master_chain?}]} |
| `plugin.params` | Every parameter of a strip, by device: the mixer (volume, pan, mute, sends), each built-in effect, the instrument and each plug-in, with its range and value now. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | {track, devices: [{device: mixer\|insert:N\|plugin:<uid>, name, running, params: [{param, name, min, max, value, text, unit?, stepped?, choices?}]}]}; with effect or instrument, only that device |
| `plugin.window` | Opens or closes the window of an instrument or effect. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect<br>open (boolean): true opens (the default), false closes | {plugin\|effect, open} |

### instrument

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `instrument.set` | Loads an instrument on an instrument track, in place of the one it has. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>id (string): the instrument: an id or name from plugin.list<br>soundfont (string): instead of id: the path of a SoundFont (.sf2) file<br>bank (integer, 0 to 16383): with soundfont: the preset's bank (128 for drum kits)<br>program (integer, 0 to 127): with soundfont: the preset's program<br>open_window (boolean): also open the instrument's window on the Mac's screen (default false) | {track, instrument: {ref, id, name, format, bypass}} |
| `instrument.remove` | Takes the instrument off a track. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1) | {track, removed} |

### effect

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `effect.add` | Adds an effect at the end of a strip's effects (a track's, or the Stereo Out's). | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**id** (string): the effect: an id or name from plugin.list ("builtin:Compressor", "Channel EQ", an Audio Unit's id ...)<br>open_window (boolean): also open the effect's window (default false) | {track, effect: {ref, id, name, format, bypass}, effects} |
| `effect.remove` | Removes an effect from a strip; its automation goes with it. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**effect** (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name | {track, removed, effects} |
| `effect.bypass` | Bypasses an effect or turns it back on. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**effect** (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>on (boolean): true bypasses, false turns it on (default: the other state) | {track, effect} |
| `effect.move` | Moves an effect up or down among the strip's effects of its kind. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**effect** (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>**by** (integer, -64 to 64): slots to move: negative is earlier in the chain | {track, effect, effects} |

### param

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `param.get` | One parameter's value now, in its own units. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**param** (string): the parameter: its key from plugin.params ("volume", "pan", "mute", "send:0", "fx:0:3", "plugin:1:42") or its name<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | {param, name, min, max, value, text, unit?, device, automation_points} |
| `param.set` | Sets a parameter, as a hand on its control does: the fader, pan, mute, a send, a built-in or native effect's knob, or a running Audio Unit's or VST3's parameter. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**param** (string): the parameter: its key from plugin.params ("volume", "pan", "mute", "send:0", "fx:0:3", "plugin:1:42") or its name<br>**value** (number): the new value in the parameter's own units (plugin.params gives its range; dB for volume, -1..1 for pan, 0 or 1 for mute)<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | the parameter as param.get gives it, with `set`; a parameter that has automation is overridden by the move, as in the app |

### preset

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `preset.list` | The presets of a plug-in: an Audio Unit's factory presets, or a native effect's. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | {plugin, presets: [{number, name}], current} |
| `preset.load` | Loads one of a plug-in's presets. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect<br>name (string): the preset's name<br>number (integer): or its number from preset.list | {loaded: {number, name}} |

### automation

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `automation.get` | The automation of a strip: one parameter's breakpoints, or every lane that has any. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>param (string): only this parameter (key or name)<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | {track, lanes: [{param, name, unit, min, max, points: [{at: {bar, position, beat, seconds}, value, curve}]}]}; values are in the parameter's own units |
| `automation.set` | Writes automation breakpoints for a parameter of a strip. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>**param** (string): the parameter: its key from plugin.params ("volume", "pan", "mute", "send:0", "fx:0:3", "plugin:1:42") or its name<br>**points** (array): the breakpoints: [{"bar": 1, "value": -6}, ...]; each has a place (bar, position, seconds or beat) and a value in the parameter's own units; a point at a place that has one replaces its value<br>replace (boolean): empty the lane first (default false: the points are added to it)<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | {track, lane} |
| `automation.clear` | Empties one automation lane of a strip, or all of them. | **track** (any): the channel strip: a track's id, name or number, or "master" for the Stereo Out<br>param (string): the lane to empty (key or name)<br>all (boolean): true: every lane of the strip<br>effect (string): an effect of the strip: "insert:1" (a built-in effect by its slot, from 1), "plugin:<uid>" (as state.get gives it) or its name<br>instrument (boolean): true: the track's instrument instead of an effect | {track, cleared} |

### midi

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `midi.note` | Plays a note on a track's instrument now, as a key of the Piano Roll's keyboard does (it is heard, not recorded into a region). | track (any): the track (default: the selected one)<br>**pitch** (integer, 0 to 127): the MIDI note, 60 is middle C<br>velocity (integer, 1 to 127): how hard (default 100)<br>on (boolean): true starts the note (the default), false ends it | {pitch, on, audio_running}; every note started must be ended |
| `midi.panic` | Ends every sounding note (Record > Send All Notes Off). | none | {sent} |

### midi_fx

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `midi_fx.list` | The MIDI FX of a track (Track > MIDI FX…), in the order the notes go through them. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1) | {track, effects: [{index, kind, on, settings: [numbers], arpeggiator?: {rate, mode, octaves, gate, swing, step_velocity, latch, trigger, length, steps}}]} |
| `midi_fx.add` | Adds a MIDI FX at the end of a track's chain, with its start settings. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>**kind** (string: scripter / modulator / transposer / velocity / scale / chord / repeater / arpeggiator / randomizer / swing / euclid): which effect<br>on (boolean): false adds it switched off (default true) | the chain, as midi_fx.list gives it |
| `midi_fx.remove` | Takes a MIDI FX out of a track's chain. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>**index** (integer): the effect's place in the track's MIDI FX, from 1 | the chain, as midi_fx.list gives it |
| `midi_fx.set` | Switches a MIDI FX on or off, or sets its settings as the numbers midi_fx.list gives (the Arpeggiator's are easier by name: midi_fx.arpeggiator). | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>**index** (integer): the effect's place in the track's MIDI FX, from 1<br>on (boolean): on or off<br>settings (array): the effect's settings as numbers, in the order midi_fx.list gives them; missing ones go to their start values | the effect, as in midi_fx.list |
| `midi_fx.arpeggiator` | Sets the Arpeggiator of a track (the first one in its MIDI FX, or the one at `index`; a track without one gets one). It plays the keys held on the track, live or from its regions, through a pattern of up to 32 steps. Only what is named changes. | **track** (any): the track: its id ("t3"), its name, or its number in the list (from 1)<br>index (integer): the effect's place in the track's MIDI FX, from 1<br>on (boolean): the effect on or off<br>rate (string: 1/4. / 1/4 / 1/4T / 1/8. / 1/8 / 1/8T / 1/16. / 1/16 / 1/16T / 1/32. / 1/32 / 1/32T): the length of a step ("." dotted, "T" triplet)<br>mode (string: up / down / up_down / down_up / random / as_played / chord): the order the held keys are played in; chord plays them all on every step<br>octaves (integer, 1 to 4): how many octaves the keys are played over<br>gate (integer, percent, 5 to 200): how long a note lasts, of a step<br>swing (integer, percent, 0 to 100): how late every second step is: 100 is half a step, 67 a triplet feel<br>step_velocity (integer, percent, 0 to 100): how much of a note's velocity comes from its step (0: all from the key played)<br>latch (boolean): keys stay held after they are let go, until new ones are pressed<br>trigger (string: restart / song_position): restart: each new chord starts the pattern again; song_position: the steps stay on the song's grid<br>length (integer, 1 to 32): the steps played before the pattern starts again<br>preset (string: Straight / Accents / Offbeat / Gallop / Three Three Two / Long Short / Octave Jump / Fifths / Stabs / Ramp): a built-in step pattern (it sets the steps and the length)<br>randomize (boolean): true: new random steps for the length<br>steps (array): steps to change: [{"step": 1, "on": true, "velocity": 100, "tie": false, "chord": false, "transpose": 0}, ...]; step is its number from 1, tie holds the note through the next step, chord plays all the held keys, transpose is semitones from -24 to 24; what is left out stays | the effect: {index, kind, on, settings, arpeggiator: {rate, mode, octaves, gate, swing, step_velocity, latch, trigger, length, steps: [{step, on, velocity, tie, chord, transpose}]}} |

### kits

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `kits.status` | The drum-kit library: its folders (present: the folder is there now), the number of kits (listed: those whose files are read and kept; pools: sources of one-sample folders shown as one) and the sources. The first call reads the folders. | none | {folders: [{path, present, kits}], kits (with the kit list active: the kits it shows), all_kits, kit_list: {path, on, active, file, rows, matched, types, error}, listed, pools, sources: [{name, kits}], scanning, listing: {done, total} or null, indexing, scan_ms, last_listing: {read, ms}, favorites, audition, default_kit, copy_kits (remote, always), copying (a kit load waits for its samples to be copied here), local} |
| `kits.rescan` | Reads the library folders again and waits for it (scan_ms is how long it took); then every kit's files are read again in the background (listing in kits.status while they are). | full (boolean): false: read again only the kits never read and those whose folder has changed (default true) | {folders: [{path, present, kits}], kits (with the kit list active: the kits it shows), all_kits, kit_list: {path, on, active, file, rows, matched, types, error}, listed, pools, sources: [{name, kits}], scanning, listing: {done, total} or null, indexing, scan_ms, last_listing: {read, ms}, favorites, audition, default_kit, copy_kits (remote, always), copying (a kit load waits for its samples to be copied here), local} |
| `kits.folders` | Sets the library folders (saved), then scans them. A folder that is not there is kept and shows present: false. | **folders** (array): the folders, a list of paths | {folders: [{path, present, kits}], kits (with the kit list active: the kits it shows), all_kits, kit_list: {path, on, active, file, rows, matched, types, error}, listed, pools, sources: [{name, kits}], scanning, listing: {done, total} or null, indexing, scan_ms, last_listing: {read, ms}, favorites, audition, default_kit, copy_kits (remote, always), copying (a kit load waits for its samples to be copied here), local} |
| `kits.search` | The kits whose source or name holds every word of the query, as the Kit Browser lists them: with the kit list active, only its kits (and the favourites and the kit of new racks) in its order, grouped by type. | query (string): words to look for (left out: every kit)<br>source (string): only kits of this source<br>best (boolean): only kits the library's BEST.md ranks, in its order<br>favorites (boolean): only the favourites<br>list (boolean): false: every kit, as if the kit list were off (default: the kit list when it is active)<br>type (string): with the kit list: only kits of this type of it<br>limit (integer, 1 to 1000): most kits to return (default 50)<br>offset (integer, 0 to 10000000): kits to skip (default 0) | {total, offset, kits: [{path, source, name, files, license, best, favorite, channels, pool, rank, type}]} |
| `kits.get` | A kit's files (from its kept listing, else read from its folder): its audio and SoundFont files with the class of each (kick, snare, clap, closed_hat, open_hat, rim, tom_low, tom_mid, tom_high, perc, shaker, crash, ride, 808, fx, loop, other, sf2), the channels a load fills and the samples its audition plays. | **path** (string): the kit's folder (a pool's: its source's folder)<br>offset (integer, 0 to 100000000): samples to skip (default 0)<br>limit (integer, 0 to 100000000): most samples to list (default 200) | {path, kind (files, sfz, hydrogen, drumgizmo), files, classes: [{class, files}], offset, samples: [{index, file, name, class}], channels: [{channel, file, class}], audition: [file]} |
| `kits.load` | Loads a kit into a Channel Rack: its samples go on the channels kits.get lists (copied to this computer first as kits.copy says); levels, pans, mutes and steps stay. A kit of SoundFonts alone goes on a new instrument track as its SoundFont (its drum preset when it has one). | **path** (string): the kit's folder<br>track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack, else a new Channel Rack track | {track, kit, channels: [{channel, name, path, class}]} |
| `kits.assign` | Puts one audio file on one channel of a Channel Rack (empty channels are added up to it). | **file** (string): the audio file<br>**channel** (integer, 1 to 64): the channel, from 1<br>track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack, else a new Channel Rack track | {track, kit, channels: [{channel, name, path, class}]} |
| `kits.audition` | Plays one sample, or a kit's kick, snare and hat, on the audition voice as the Kit Browser does. With capture, nothing is played aloud: the audition voice's output is written to that WAV file instead and compared with the files played. | file (string): one audio file<br>path (string): a kit's folder (its kick, snare and hat)<br>capture (string): a WAV file to write the audition into instead of playing it | {playing: [file]}, or with capture {capture, rate, frames, seconds, peak, rms, hits: [{file, at_seconds, correlation}]} |
| `kits.copy` | When a kit or sample loaded into a rack is copied to this computer (kit-library/local/<source>/<kit>/, the rack then plays the copies): remote copies only from folders on a network share (the default), always from every folder. Left out: tells the setting. | mode (string: remote / always): remote or always (saved) | {mode, local} |
| `kits.list` | The kit list: a TSV (header with path, and rank, type, reason) naming kits by their folder relative to the library folder. Set, the Kit Browser, kits.search and kits.status show only its kits (and the favourites and the kit of new racks), in rank order, grouped by type. A list path not found as written is looked for in the library folders (the same file on another Mac's mount). Left out: tells the setting. | path (string): the TSV (saved; turns the list on); "" clears it<br>on (boolean): false: the list is kept but not used (saved), as the Kit Browser's Kit List switch | {path, on, active (set, read and on), file (the file read), rows, matched (kits of the list found), types: [{type, kits}], error} |
| `kits.favorite` | Marks a kit as a favourite, or takes the mark off. | **path** (string): the kit's folder<br>on (boolean): false takes the mark off (default true) | {path, favorite} |

### rack

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `rack.get` | A Channel Rack: each channel's sample, drum class, swing and steps (in the region the rack's window shows, with where each step's note starts), and the feel: the swing and sixteen offsets of each class that has its own, and of every other class. A feel is {swing, sub, offsets, offset (percent of the offsets the class plays, under the rack's offset amount)}. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack | {track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]} |
| `rack.class` | Gives a channel another drum class; its steps move to that class's feel. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**channel** (integer, 1 to 64): the channel, from 1<br>**class** (string: kick / snare / clap / closed_hat / open_hat / rim / tom_low / tom_mid / tom_high / perc / shaker / crash / ride / 808 / fx / other): the class | {track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]} |
| `rack.swing` | Sets the swing of one drum class (or of all) and moves the steps of its channels; the offsets stay. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**class** (string): the class, or all<br>**pct** (number, %, 35 to 85): the off-beat's place in its pair: 50 straight, 66.7 triplet<br>sub (string: 16th / 8th): which off-beats swing: 16th (the e and a) or 8th (the and); left out: as it is | the rack.get answer, with moved (notes moved) |
| `rack.offset` | Sets how much of its offsets one drum class (or every class) plays: the drum's own Offset, under the rack's Offset amount (rack.amounts offset), which scales every class. The steps of its channels move. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**class** (string): the class, or all<br>**pct** (number, %, 0 to 100): 0: on the swung step, 100: the class's offsets as loaded | the rack.get answer, with moved (notes moved) |
| `rack.steps` | Sets which steps of a channel are on, in the region the rack's window shows; each note goes where the channel's feel plays its step. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**channel** (integer, 1 to 64): the channel, from 1<br>**steps** (array): the step numbers on, from 1 (the others go off) | {track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]} |
| `rack.groove` | Sets the rack's whole feel (as rack.get gives it), or makes it straight; the steps move. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>groove (any): {name, source, default: feel, classes: {class: feel}}, a feel {swing, sub (16th, 8th), offsets: [16 quarter notes, one a sixteenth of the bar], offset (percent of them played, default 100)}; null or left out: straight | the rack.get answer, with moved (notes moved) |
| `rack.mix` | Sets a channel's volume, pan, mute or solo (what its knobs and its Mute and Solo keys set). | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**channel** (integer, 1 to 64): the channel, from 1<br>gain_db (number, dB, -48 to 12): the volume<br>pan (number, -1 to 1): the pan: -1 left, 0 centre, 1 right<br>mute (boolean): muted<br>solo (boolean): soloed: while any channel is, only the soloed ones play | {track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]} |
| `rack.amounts` | Sets how much of the rack's groove plays: the swing (0: straight, 100: as loaded or set), the offsets, and the source's velocities (a loaded pattern's or groove's; 0: a pattern's hits at the rack's velocity when it was loaded, a groove's steps at their velocity before); and the rack's velocity (of a hit added by hand). The steps move and take their velocities. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>groove (number, %, 0 to 100): how much of each class's swing plays<br>offset (number, %, 0 to 100): how much of the offsets plays, over every class's own offset amount (rack.offset)<br>velocity (number, %, 0 to 100): how much of the loaded pattern's or groove's velocities plays<br>new_velocity (integer, 1 to 127): the rack's velocity | the rack.get answer, with moved (notes moved) |
| `rack.hit` | Turns one hit of a channel on or off, in the region the rack's window shows: a whole step (div 1) or part sub of a step split in div (2: 32nds, 3: triplets, 4: 64ths), with its velocity. A hit of the loaded pattern keeps the velocity it now stands for. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**channel** (integer, 1 to 64): the channel, from 1<br>**step** (integer, 1 to 64): the step, from 1<br>div (integer, 1 to 4): parts the step is split into (default 1)<br>sub (integer, 0 to 3): the part, from 0 (default 0)<br>on (boolean): on (default) or off<br>vel (integer, 1 to 127): its velocity (left out: the pattern's there, else the rack's) | {track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]} |
| `rack.divide` | Splits a step of a channel into hits (a ratchet: 1 whole, 2 32nds, 3 triplets, 4 64ths), each at the loudest velocity the step had. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**channel** (integer, 1 to 64): the channel, from 1<br>**step** (integer, 1 to 64): the step, from 1<br>**div** (integer, 1 to 4): how many hits | {track, region (null: none yet), kit, steps, groove: {name, source, straight, default: feel, classes: {class: feel}}, amounts: {groove, offset, velocity (percent)}, velocity (of a hit added by hand), pattern: {name, source, steps, hits}, copying (a kit being copied here), channels: [{channel, note, name, path, class, gain_db, pan, mute, swing, offset (percent of its class's offsets played), steps: [from 1], starts: [quarter notes from the region's start], hits: [{step, sub, div, vel, start}]}]} |
| `rack.pattern` | Loads a pattern given as hits by drum class into a Channel Rack, onto the kit there: each hit on the first channel of its class (else of a class that stands in, else a new channel of the class), in the rack's region made as long as the pattern; its velocities are kept for the velocity amount. The groove stays, its amounts at 100 %. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack<br>**hits** (array): [{class, step (from 1), div (1-4, default 1), sub (from 0), vel (default 100)}]; instead of step/div/sub a beat (quarter notes from the start) goes to the nearest step or part of one<br>steps (integer, 1 to 64): how many steps long (default 16)<br>name (string): its name | the rack.get answer, with loaded: {hits, dropped, channels_added} |
| `rack.split_tracks` | Split to Tracks: puts each channel of the rack that has a sample on an instrument track of its own named for its drum class (Kick, Snare, a second kick Kick 2; a class without a name: the sample's), right under the rack's track, playing a Channel Rack with that channel alone (its sample, volume, pan, mute and class, the rack's groove and amounts). The channel's notes move there unchanged, on the new rack's channel 1, in regions where the rack's are. The rack's track stays, muted and named "<name> (split)". One Undo step. | track (any): the rack's track: its id ("t3"), its name, or its number in the list (from 1); left out: the selected track when it plays the Channel Rack | {track (the rack's), name, tracks: [{track, name, channel (the rack's, from 1), notes (moved)}]} |
| `rack.follow_get` | Follow Rack Groove of a MIDI track: whether its notes play on the hits of one drum class of a Channel Rack where the rack plays them (a loaded pattern's at their played times; a note where the class plays no hit stays), which rack, which class (Auto picks kick for 808s and bass, the closed hat for dense sixteenths, else the snare) and whether the hits' velocities accent the notes. The notes stay where they are stored; playback, the bounce and the MIDI export (unless switched off) take the rack's timing. | **track** (any): the MIDI track: its id ("t3"), its name, or its number in the list (from 1) | {track, on, rack (from 1), rack_track (null: no rack), class (the choice: auto or a class), auto (what Auto picks for the track), follows (the class it plays with now; null: off or no rack), velocity, groove_q (Groove Q, 0-100 %), live (Groove Live), racks: [{rack, track, name}]} |
| `rack.follow` | Sets Follow Rack Groove of a MIDI track (the parameters left out stay as they are). | **track** (any): the MIDI track: its id ("t3"), its name, or its number in the list (from 1)<br>on (boolean): follow a rack<br>rack (integer, 1 to 256): which rack: 1 the first track playing a Channel Rack, 2 the second…<br>class (string: auto / hat / kick / snare / clap / closed_hat / open_hat / rim / tom_low / tom_mid / tom_high / perc / shaker / crash / ride / 808 / fx / other): the class whose hits the notes play on: auto, hat (the closed hat), snare, kick, or another rack class<br>velocity (boolean): the velocity of the rack's hit carrying each note, over the mean of the class's hits, scales the note's | {track, on, rack (from 1), rack_track (null: no rack), class (the choice: auto or a class), auto (what Auto picks for the track), follows (the class it plays with now; null: off or no rack), velocity, groove_q (Groove Q, 0-100 %), live (Groove Live), racks: [{rack, track, name}]} |
| `rack.groove_q_get` | Groove Q of a MIDI track: a second quantize, toward a Channel Rack's hits instead of the straight grid. Each note is pulled from where it is toward the target Follow Rack gives the nearest step (the region's Quantize grid, sixteenths when off) for the class Follow picks: on a step the class has a hit on, where that one hit plays (a loaded pattern's at its played time, never a mean); between two hits at most a sixteenth apart, its share of the way between them; where the class plays no hit the note stays. By the strength. A region takes its track's strength unless it has its own. The notes stay where they are stored; playback, the bounce and the MIDI export take it. | **track** (any): the MIDI track: its id ("t3"), its name, or its number in the list (from 1) | {track, strength (0-100 %), live, rack (from 1), rack_track (null: no rack), class (the choice), pulls_to (the class it pulls to now; null: no rack), follow_rack (Follow Rack on), regions: [{region, own (null: the track's), strength}]} |
| `rack.groove_q` | Sets Groove Q of a MIDI track, or with `region` of one of its regions (the parameters left out stay as they are). | **track** (any): the MIDI track: its id ("t3"), its name, or its number in the list (from 1)<br>strength (integer, %, 0 to 100): how far the notes are pulled to the groove: 0 (off) to 100 (on it); the track's, or the region's with `region`<br>region (any): a region of the track (id "r12" or name): `strength` and `track_strength` are its own<br>track_strength (boolean): with `region`: true makes the region take its track's strength again<br>live (boolean): Groove Live: keys played live while the song plays wait for a step the groove plays late, by the strength (never earlier); the take is recorded where the keys were pressed<br>rack (integer, 1 to 256): which rack: 1 the first track playing a Channel Rack, 2 the second…<br>class (string: auto / hat / kick / snare / clap / closed_hat / open_hat / rim / tom_low / tom_mid / tom_high / perc / shaker / crash / ride / 808 / fx / other): the class whose hits the notes are pulled to (shared with Follow Rack) | {track, strength (0-100 %), live, rack (from 1), rack_track (null: no rack), class (the choice), pulls_to (the class it pulls to now; null: no rack), follow_rack (Follow Rack on), regions: [{region, own (null: the track's), strength}]} |
| `rack.groove_flex_get` | Groove Flex of an audio track: Flex Time toward a Channel Rack's groove. The hits of each audio region are found once (in the background) and each moves toward the target Follow Rack gives the nearest sixteenth (the region's Quantize grid when set) for the class Follow picks (where that step's own hit plays; a hit with no rack hit there stays) (Auto: Kick for 808 and bass by name, the closed hat for dense hits, else the snare), by the strength; the sound between hits is stretched without changing its pitch (WSOLA, the region's Flex mode, Rhythmic when it had none), the region keeps its length and the recording is never changed (0 %: it plays as recorded). Playback and the bounce take it once the sound is made (`busy` false). | **track** (any): the audio track: its id ("t3"), its name, or its number in the list (from 1) | {track, strength (0-100 %), rack (from 1), rack_track (null: no rack), class (the choice), pulls_to (the class it pulls to now; null: no rack), regions: [{region, own (null: the track's), strength, hits (null: not found yet), markers, mode, finding_hits}], busy (the hits being found or the sound being made)} |
| `rack.groove_flex` | Sets Groove Flex of an audio track, or with `region` of one of its regions (the parameters left out stay as they are). Setting it to 0 removes the Groove Flex markers. | **track** (any): the audio track: its id ("t3"), its name, or its number in the list (from 1)<br>strength (integer, %, 0 to 100): how far the hits are pulled to the groove: 0 (off) to 100 (on it); the track's, or the region's with `region`<br>region (any): an audio region of the track (id "r12" or name): `strength` and `track_strength` are its own<br>track_strength (boolean): with `region`: true makes the region take its track's strength again<br>rack (integer, 1 to 256): which rack: 1 the first track playing a Channel Rack, 2 the second…<br>class (string: auto / hat / kick / snare / clap / closed_hat / open_hat / rim / tom_low / tom_mid / tom_high / perc / shaker / crash / ride / 808 / fx / other): the class whose hits the audio hits are pulled to | {track, strength (0-100 %), rack (from 1), rack_track (null: no rack), class (the choice), pulls_to (the class it pulls to now; null: no rack), regions: [{region, own (null: the track's), strength, hits (null: not found yet), markers, mode, finding_hits}], busy (the hits being found or the sound being made)} |

### region

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `region.get` | One region: where it is, how long, and its notes or its audio file. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>notes (boolean): include a MIDI region's notes (default true) | {id, track, name, kind: midi\|audio\|pattern\|drummer, start_bar, length_bars, start_beat, length_beats, start_seconds, mute, loop_beats, and for MIDI: transpose, velocity_offset, quantize_beats, note_count, take_folder, notes: [{pitch, start, length, velocity, mute?, lyric?}]; for audio: audio: {file, file_seconds, sample_rate, channels, flex, offset_seconds, gain_db, fade_in_beats, fade_out_beats, reverse}} |
| `region.create` | Makes a MIDI region on a track, empty or with notes. | **track** (any): the track: id, name or number<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>length_bars (number, bars, 0 to 100000): its length in bars (default: as long as its notes, in whole bars; 4 bars when empty)<br>length_beats (number, quarter notes, 0 to 10000000): or its length in quarter notes<br>name (string): its name (default: the track's)<br>notes (array): the notes: [{"pitch": 60, "start": 0, "length": 1, "velocity": 100, "lyric": "la"}, ...]; start and length in quarter notes from the region's start; velocity (default 100), lyric and mute may be left out<br>select (boolean): select it, so the editors show it (default true) | the new region as region.get gives it, without its notes |
| `region.notes.set` | Replaces all the notes of a MIDI region. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>**notes** (array): the notes: [{"pitch": 60, "start": 0, "length": 1, "velocity": 100, "lyric": "la"}, ...]; start and length in quarter notes from the region's start; velocity (default 100), lyric and mute may be left out<br>grow (boolean): lengthen the region to the bar that holds the last note (default true) | the region, with grown |
| `region.notes.add` | Adds notes to a MIDI region, keeping the ones it has. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>**notes** (array): the notes: [{"pitch": 60, "start": 0, "length": 1, "velocity": 100, "lyric": "la"}, ...]; start and length in quarter notes from the region's start; velocity (default 100), lyric and mute may be left out<br>grow (boolean): lengthen the region to the bar that holds the last note (default true) | the region, with grown |
| `region.move` | Moves a region in time and/or to another track, as dragging it does (its automation goes along when the app is set to move it with regions). | **region** (string): the region: its id ("r12") or its name when only one region has it<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>track (any): the track to move it to (default: it stays on its track)<br>crossfade (boolean): audio: crossfade with a region it lands on | the region where it is now |
| `region.copy` | Copies a region to another place and/or track, as Option-dragging it does. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>track (any): the track the copy goes on (default: the region's own)<br>crossfade (boolean): audio: crossfade with a region it lands on | the copy |
| `region.resize` | Changes a region's length, as dragging its right edge does. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>length_bars (number, bars, 0 to 100000): the new length in bars<br>length_beats (number, quarter notes, 0 to 10000000): or in quarter notes<br>end_bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>end_position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>end_seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>end_beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | the region |
| `region.split` | Cuts a region in two at a place (the Scissors tool). | **region** (string): the region: its id ("r12") or its name when only one region has it<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | {left, right} |
| `region.join` | Joins regions of one track into one (Edit > Join). | **regions** (array): the regions to join: ids or names | the joined region |
| `region.loop` | Loops a region: it plays again and again after its end. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>repeats (number, 0 to 4096): how many times it plays again after the first<br>end_bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>end_position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>end_seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>end_beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>off (boolean): true: no loop | the region; loop_beats is how long the loop runs after the region's end |
| `region.set` | Sets a region's name, mute and Region Inspector values. | **region** (string): the region: its id ("r12") or its name when only one region has it<br>name (string): its name<br>mute (boolean): muted regions do not play<br>loop_beats (number, quarter notes, 0 to 10000000): how long the loop runs after the region's end; 0 is no loop<br>transpose (integer, semitones, -96 to 96): MIDI: semitones<br>velocity_offset (integer, -99 to 99): MIDI: added to every note's velocity<br>quantize_beats (number, 0 to 16): MIDI: playback quantize grid in quarter notes (0.25 is a sixteenth); 0 is off<br>gain_db (number, dB, -96 to 24): audio: region gain<br>fade_in_beats (number, quarter notes, 0 to 1000000): audio: fade in<br>fade_out_beats (number, quarter notes, 0 to 1000000): audio: fade out<br>reverse (boolean): audio: play it backwards | the region |
| `region.delete` | Deletes a region. | **region** (string): the region: its id ("r12") or its name when only one region has it | {deleted, name} |
| `region.fit_to_grid` | Fits an audio region to the grid where it is (Region menu > Flex Time > Fit to Grid). It sets the project's tempo when it is the project's only audio. | **region** (string): the region: its id ("r12") or its name when only one region has it | {region, fitting, status}; the work runs in the background (event import.fitted) |

### import

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `import.audio` | Imports an audio file with the app's own import: a new audio track (or a track that is there) gets the file as a region at the bar of the place given. | **path** (string): the audio file (WAV, AIFF, FLAC, MP3, CAF): an absolute path, or one starting with ~/<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>track (any): an audio track to put it on (default: a new audio track named after the file)<br>fit_to_grid (boolean): Fit to Grid: the project's first audio gives it its tempo, later files are stretched to it; either way onto the grid (default: the app's File > Import > Fit to Grid on Drop setting) | the new region, with fit_to_grid and fitting; without a place it goes at the bar under the playhead |
| `import.midi` | Imports a MIDI file: each of its tracks becomes a new track with one region. | **path** (string): the .mid file<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | {tracks_made, tracks}; a place puts the file's start there (default: the start of the song) |

### export

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `export.midi` | Writes the project's MIDI, or one region's, to a MIDI file. | **path** (string): the .mid file to write<br>region (string): only this region (default: every MIDI region)<br>overwrite (boolean): replace a file that is already there<br>apply_follow_groove (boolean): write the notes of tracks following a Channel Rack with its feel, as they play (default true); false: as stored, on the straight grid | {path, whole_project, apply_follow_groove} |
| `export.bounce` | Bounces the project to an audio file (File > Bounce > Project or Section): everything the app plays, through a second engine that has no sound card, faster than it plays. Audio regions, instruments, Aux strips and sends, effects, automation, tempo changes and the Stereo Out are in it; Mute and Solo are as in the app. Without a range: the cycle range while Cycle is on, else from bar 1 to the end of the last region. With `stems`, every track that is heard is also written to a file of its own beside the mix ("<mix> - 03 <track>.wav"), each from the same start and through the Stereo Out. `not_rendered` lists what should be in the file and is not, with the reason; `complete` says there is none. The call is answered when the bounce ends: pass `--timeout 0` to prowavectl for a long song. | path (string): the file of the mix, ending in .wav or .aiff (default: a numbered WAV file in the session's folder)<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>end_bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>end_position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>end_seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>end_beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>format (string: wav / aiff): the file format (default: what the path's ending names)<br>bit_depth (integer): 16 or 24 (whole numbers) or 32 (float, WAV only); default 24<br>normalize (boolean): raise or lower each file so its loudest sample is at full scale (default false)<br>tail_seconds (number, seconds, 0 to 600): sound kept after the end, for reverb and release. Given: exactly this much. Not given: one second, and on while the output is above -80 dB, up to `max_tail_seconds`<br>max_tail_seconds (number, seconds, 0 to 600): the longest tail when it follows the sound (default 20; with `tail_seconds` given, the tail may grow up to here)<br>mix (boolean): write the mix (default true)<br>stems (boolean): also write every track that is heard into a file of its own (default false)<br>track (any): only this track, into `path` (the others play nothing; `mix` and `stems` are not looked at)<br>overwrite (boolean): replace files that are already there<br>wait (boolean): answer when the bounce ends (default true); false answers at once with {bounce, started, path} and the end comes as the event bounce.finished, or ask export.bounce.status | {bounce, path, files: [{path, track, name, seconds, frames, peak_db, rms_db, silent, tail_seconds, gain_db, clipped_samples, speed}], seconds, sample_rate, channels, format, from, to, range_seconds, tail_seconds, peak_db, rms_db, silent, rendered_tracks, muted_tracks, not_rendered, complete, speed, took_seconds}; the top-level path, seconds and levels are the first file's (the mix); clipped_samples counts samples at full scale (the output is cut off at 0 dBFS, as the app's is); speed is seconds of sound for each second of rendering |
| `export.bounce.status` | How far the running bounce is, or how the last one ended. | none | {running, bounce, fraction, file, files, track, preparing} while one runs; {running: false, bounce, ok, result \| error} after |
| `export.bounce.cancel` | Stops the running bounce; the file being written is removed and the waiting export.bounce call gets an error. | none | {cancelled, bounce} |

### marker

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `marker.list` | The markers, in song order. | none | {markers: [{number, name, at, color}]} |
| `marker.add` | Adds a marker at a place (default: the playhead). | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>name (string): its name (default "Marker N") | {marker: its number, markers} |
| `marker.set` | Renames a marker and/or moves it. | **marker** (any): the marker: its number (from 1, in song order) or its name<br>name (string): a new name<br>bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | {markers} |
| `marker.delete` | Deletes a marker. | **marker** (any): the marker: its number or its name | {deleted, markers} |

### tempo

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `tempo.list` | The Tempo track's points and the Signature track's changes. | none | {tempo_changes: [{at, bpm, ramp}], signatures: [{bar, numerator, denominator}]} |
| `tempo.add` | Adds a tempo change at a place (or replaces the one there). | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>**bpm** (number, bpm, 5 to 990): the tempo from there on<br>ramp (boolean): this point ramps to the next one (a tempo curve) | {tempo_changes} |
| `tempo.remove` | Removes the tempo change at a place. | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based | {tempo_changes} |

### signature

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `signature.remove` | Removes the signature change that starts at a bar (transport.signature adds one). | **bar** (integer, 2 to 9999): the bar the change starts at (not 1) | {signatures} |

### menu

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `menu.list` | Every menu command the app shows, with whether it can be chosen now. | menu (string): only this menu (File, Edit, Track, Navigate, Record, Mix, View, Window, Help) | {entries: [{id, title, menu, submenu?, enabled, checked?, opens_system_window?, use_instead?}], open_recent: [paths]}; an id is the entry's English title |
| `menu.invoke` | Chooses a menu command, as a click on it does. | **id** (string): the entry's id from menu.list (its English title; the trailing … may be left out)<br>allow_system_window (boolean): also choose an entry that opens a system file window (the app then waits for a person) | {invoked, checked?}; changes made by the entry get their Undo step within half a second, as menu clicks do |

### ui

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `ui.dialog` | The question the app is asking in a modal window, if any. | none | {dialog: null \| {id: save_changes\|revert\|missing_audio\|recover, text, buttons, detail}} |
| `ui.dialog.answer` | Answers the open question, as a click on one of its buttons does. | **button** (string): one of the buttons ui.dialog lists (save, dont_save, cancel, revert, ok, recover, discard)<br>path (string): for save on a project that has no file yet: the .prowave file to save to | {answered, button, dialog: the next open question or null} |
| `ui.windows` | The app's panels and windows and which are open. | none | {windows: [{id, open}], bottom_panel, project_chooser} |
| `ui.window` | Opens or closes a panel or window of the app. | **id** (string: mixer / piano_roll / score / channel_rack / pitch_editor / piano_roll_window / inspector / list_editors / toolbar / notes / tips / midi_monitor / midi_fx / free_plugins / kits / project_settings / tempo_track / marker_track / signature_track / master_track / automation): which one<br>open (boolean): true to open (the default), false to close<br>tab (string: general / synchronization / metronome / recording / tuning / audio / midi): for project_settings: the tab to show | {id, open} |
| `ui.view` | Zooms and scrolls the Tracks area. | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>end_bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>end_position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>end_seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>end_beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>fit (boolean): zoom out to show every region (Navigate > Zoom to Fit)<br>bar_width (number, points, 6 to 600): width of one bar<br>track_height (number, points, 20 to 400): height of a track<br>follow_playhead (boolean): scroll with the playhead while playing (Catch) | {bar_width, track_height, first_bar, follow_playhead, lanes_width}; a start place scrolls there, a start and an end zoom to that range |
| `ui.select` | Selects a track and/or regions, as clicks in the Tracks area do. | track (any): the track to select: id, name or number<br>regions (array): the regions to select (ids or names); the first is the one the Inspector and the editors show<br>all_regions (boolean): select every region (Edit > Select All)<br>none (boolean): select no region | {track, regions} |
| `ui.status` | The app's status line (import errors, engine warnings) and the load meters. | clear (boolean): empty the status line after reading it | {status, cpu_load, playback_load, overloads, audio_running} |
| `ui.screenshot` | Saves a picture of the app's window as a PNG, also while the app is in the background. | path (string): where to write the .png (default: a numbered file in the session's folder, which is deleted when the app quits)<br>scale (number, 0.05 to 1): size of the picture relative to the window's pixels (default 0.5)<br>window (string: main / piano_roll_window): which window (default main) | {path, width, height, window}; plug-in windows are the plug-ins' own and are not in the picture |

### audio

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `audio.get` | The audio engine's device, buffer and monitoring settings, and the devices to choose from. | none | {running, output, sample_rate, buffer_frames, buffer_choices, process_buffer_frames, low_latency, software_monitoring, round_trip_ms, output_device, input_device, output_devices, input_devices} |
| `audio.set` | Changes audio settings of this Mac (not of the project). Sound may drop out while a device or the buffer changes. | buffer_frames (integer, frames): the I/O buffer (Record > I/O Buffer Size)<br>low_latency (boolean): Record > Low Latency Mode<br>software_monitoring (boolean): Record > Software Monitoring<br>output_device (string): the id of an output device from audio.get, or "default"<br>input_device (string): the id of an input device from audio.get, or "default" | the settings as audio.get gives them |

### events

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `events.subscribe` | Sends this connection a line for each thing that happens in the app, until it closes or unsubscribes. | none | {subscribed: true, events: [names]}; each event is a line {"jsonrpc":"2.0","method":"event","params":{"event":name,...}} |
| `events.unsubscribe` | Stops the events on this connection. | none | {subscribed: false} |

### app

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `app.quit` | Quits the app. What happens to unsaved changes must be said; it never waits on a question. | **if_unsaved** (string: save / discard / cancel): save: save first (to `path`, or the project's file); discard: quit without saving; cancel: do not quit when there are unsaved changes<br>path (string): for save on a project that has no file yet: the .prowave file to save to | {quitting, unsaved_changes} |

### render

| Method | What it does | Parameters | Result |
|---|---|---|---|
| `render.range` | Plays a stretch of the song into a WAV file of 32-bit floats, without the speakers, for measuring what was made. It is the app's bounce (see export.bounce) with a fixed tail: audio regions, instruments, effects, sends, automation and the Stereo Out are in it. Without a range it runs from bar 1 to the end of the last region. The call is answered when the file is written. | bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>end_bar (number, bars, 1 to 100000): a place as a bar, 1-based as the app shows it; a fraction is a part of the bar (5.5 is halfway through bar 5)<br>end_position (string): a place as "bar.beat", "bar.beat.division" or "bar.beat.division.tick", all 1-based as in the app's display ("5.3" is beat 3 of bar 5; divisions are sixteenths, 240 ticks each)<br>end_seconds (number, seconds, 0 to 10000000): a place as seconds from the start of the song<br>end_beat (number, quarter notes, 0 to 100000000): a place as quarter notes from the start of the song, 0-based<br>track (any): only this track (default: the whole mix)<br>path (string): the .wav file to write (default: a numbered file in the session's folder, deleted when the app quits)<br>overwrite (boolean): replace a file that is already there<br>tail_seconds (number, seconds, 0 to 60): sound kept after the end, for reverb and release (default 1) | {path, files: [{path, track, name, seconds, frames, peak_db, rms_db, silent, tail_seconds, gain_db, clipped_samples, speed}], seconds, sample_rate, channels, format, from, to, range_seconds, tail_seconds, peak_db, rms_db, silent, rendered_tracks, muted_tracks, not_rendered, complete, speed, took_seconds} |
<!-- methods:end -->

## Worked examples

Each line is a real command. Run against an app with a new project.

```sh
# Tempo 92, and where things are
prowavectl call transport.tempo '{"bpm": 92}'
prowavectl call transport.get

# A software instrument track with a named instrument (ids and names: plugin.list)
prowavectl call plugin.list '{"kind": "instrument", "search": "rack"}'
prowavectl call track.add '{"kind": "instrument", "name": "Keys", "instrument": "Channel Rack"}'

# An 8-bar chord part: a region at bar 1 with one chord a bar (starts in quarter notes)
prowavectl call region.create '{"track": "Keys", "bar": 1, "length_bars": 8, "name": "Chords", "notes": [
  {"pitch": 57, "start": 0, "length": 4}, {"pitch": 60, "start": 0, "length": 4}, {"pitch": 64, "start": 0, "length": 4},
  {"pitch": 53, "start": 4, "length": 4}, {"pitch": 57, "start": 4, "length": 4}, {"pitch": 60, "start": 4, "length": 4},
  {"pitch": 48, "start": 8, "length": 4}, {"pitch": 52, "start": 8, "length": 4}, {"pitch": 55, "start": 8, "length": 4},
  {"pitch": 55, "start": 12, "length": 4}, {"pitch": 59, "start": 12, "length": 4}, {"pitch": 62, "start": 12, "length": 4}]}'
prowavectl call region.loop '{"region": "Chords", "repeats": 1}'

# An audio file at bar 5, fitted to the grid (the analysis runs in the background)
prowavectl call import.audio '{"path": "~/Music/loop.wav", "bar": 5, "fit_to_grid": true}'
prowavectl call rpc.wait '{"frames": 30}'
prowavectl call state.get '{"notes": false}'      # busy.fit_to_grid is false when it is done

# An effect on the track, and one of its parameters
prowavectl call effect.add '{"track": "Keys", "id": "builtin:Compressor"}'
prowavectl call plugin.params '{"track": "Keys", "effect": "insert:1"}'
prowavectl call param.set '{"track": "Keys", "param": "volume", "value": -6}'

# Loop bars 1 to 9 and play
prowavectl call transport.cycle '{"on": true, "bar": 1, "end_bar": 9}'
prowavectl call transport.play '{"bar": 1}'
prowavectl call transport.stop

# Look at the app (also while it is behind other windows)
prowavectl call ui.screenshot '{"path": "/tmp/prowave.png"}'

# Hear it without the speakers: bars 1 to 9 as a WAV file (everything the app plays)
prowavectl call render.range '{"bar": 1, "end_bar": 9, "path": "/tmp/prowave-bars-1-9.wav"}'

# Bounce the whole project (File > Bounce > Project or Section) to a 24-bit WAV file
prowavectl --timeout 0 call export.bounce '{"path": "~/Desktop/Song.wav"}'
# ... and every track that is heard into a file of its own beside it
prowavectl --timeout 0 call export.bounce '{"path": "~/Desktop/Song.wav", "stems": true, "overwrite": true}'

# Several changes as one Undo step
prowavectl call batch '{"calls": [
  {"method": "track.add", "params": {"kind": "audio", "name": "Vox"}},
  {"method": "track.set", "params": {"track": "Vox", "volume_db": -3, "pan": 0.2}}]}'
prowavectl call project.undo

# Save
prowavectl call project.save_as '{"path": "~/Music/Pro Wave/Sketch.prowave"}'
```

Anything a menu can do can be done without a method of its own: `menu.list` gives every menu
command with its id (its English title) and whether it can be chosen now, and `menu.invoke`
chooses it.

## Questions the app asks

The app asks in a modal window before it throws unsaved work away, after a crash, and when
audio files are missing. While such a question is open, every method that changes the project
fails with the error `dialog_open`, which names the question and its buttons. `ui.dialog`
shows the open question; `ui.dialog.answer` answers it:

| `id` | Asked when | Buttons |
|---|---|---|
| `save_changes` | New, Open, Close or Quit with unsaved changes (from the app's own menus) | `save`, `dont_save`, `cancel` |
| `revert` | File > Revert to Saved | `revert`, `cancel` |
| `missing_audio` | a project was opened or saved and some audio files could not be read | `ok` |
| `recover` | at start, when a recovery copy was found | `recover`, `discard` |

The control methods themselves never open the first one: `project.new`, `project.open`,
`project.close` and `app.quit` take `if_unsaved` and fail with `unsaved_changes` when it is
needed and not given.

Menu entries that open a **system file window** (Open…, Save As…, MIDI File…) are refused by
`menu.invoke` unless `allow_system_window` is true: only a person at the Mac can answer that
window, and the app takes no requests until it is closed. Each has a method that takes a path
instead (`menu.list` names it in `use_instead`).

## Events

`events.subscribe` makes the connection receive a line for each thing that happens, until it
closes (`prowavectl events` prints them). An event is
`{"jsonrpc":"2.0","method":"event","params":{"event":"<name>", ...}}`.

| Event | Fields | When |
|---|---|---|
| `transport.started` | `playhead` | playing starts |
| `transport.stopped` | `playhead` | playing stops |
| `transport.recording` | `on` | recording starts or stops |
| `transport.bar` | `bar` | the playhead enters another bar while playing |
| `transport.tempo` | `bpm` | the tempo at the playhead changes |
| `project.changed` | `dirty` | the project's content changed (looked at four times a second) |
| `project.saved` | `path` | the project has no unsaved changes any more |
| `project.opened` | `name`, `path` | another project file is open |
| `track.added` | `id`, `name`, `kind`, `number` | a track was added |
| `track.removed` | `id`, `name` | a track was removed |
| `recording.finished` | `kind` (`audio` or `midi`), `regions` (`id`, `track`, `name`, `file`) | a recording ended; `file` is the audio file it wrote |
| `import.fitted` | `status`, `tempo` | Fit to Grid finished |
| `bounce.progress` | `bounce`, `fraction`, `file`, `files`, `track`, `preparing` | a bounce moved on by a percent, or went from preparing to rendering |
| `bounce.finished` | `bounce`, `ok`, `result` or `error` | a bounce ended; `result` is what `export.bounce` answers |
| `dialog.opened` | `dialog` | a question opened |
| `dialog.closed` | `id` | it was answered |
| `status` | `text` | the app's status line changed: import errors, engine warnings |

## Errors

An error is a JSON-RPC error object: `code` (a number), `message` (plain English: what was
wrong and what the valid values are) and `data.code` (a name that does not change), with more
fields in `data` where they help (`param`, `min`, `max`, `values`, `dialog`, `failed_index`).

| `data.code` | Number | Meaning |
|---|---|---|
| `parse_error` | -32700 | The line is not JSON. |
| `invalid_request` | -32600 | The line is JSON but not a JSON-RPC 2.0 request. |
| `method_not_found` | -32601 | No method has that name. |
| `invalid_params` | -32602 | A parameter is missing, unknown, of the wrong type or out of range. |
| `internal_error` | -32603 | The app failed while running the method (a bug). |
| `not_found` | -32000 | The track, region, effect, marker or file named does not exist. |
| `not_supported` | -32001 | The app cannot do this (here, or for this kind of thing). |
| `failed` | -32002 | The app tried and could not (the message says why). |
| `timeout` | -32003 | The work did not finish in time. |
| `busy` | -32004 | The app is doing something that must finish first. |
| `too_large` | -32005 | The request line is longer than 8 MB. |
| `unsaved_changes` | -32006 | The project has unsaved changes and the call did not say what to do with them. |
| `dialog_open` | -32007 | A question is open in the app and must be answered first. |

## The wire format

For a client other than `prowavectl`. Newline-delimited JSON-RPC 2.0 over the Unix socket:
one request a line, one response a line, any number of connections.

```
-> {"jsonrpc":"2.0","id":1,"method":"transport.locate","params":{"bar":5}}
<- {"jsonrpc":"2.0","id":1,"result":{...}}
-> {"jsonrpc":"2.0","id":2,"method":"transport.tempo","params":{"bpm":2000}}
<- {"jsonrpc":"2.0","id":2,"error":{"code":-32602,"message":"`bpm` must be from 5 to 990 bpm; got 2000","data":{"code":"invalid_params","param":"bpm","min":5.0,"max":990.0}}}
```

`params` is an object of named values (or left out). A request without `id` is a notification
and gets no answer. JSON-RPC's array batches are not taken; use the method `batch`. Requests
on one connection are answered in order, except `ui.screenshot` and `rpc.wait`, whose answers
come when their work is done.

## How it runs inside the app

- The socket has its own threads (one accepting, two a connection). They only read, parse and
  write lines. A request is handed to the interface thread through a channel and the interface
  is woken (`request_repaint`).
- The interface thread takes requests in `App::logic`, which eframe calls before every frame
  and also while the window is minimized or covered. After a request it asks for one painted
  frame even when hidden (`ViewportCommand::RequestPaintWhileHidden`), because the frame is
  where the engine is handed the change and where plug-ins are opened. So calls work while the
  app is in the background.
- A call that changes the project ends with one Undo step, made the way the app makes one when
  a mouse gesture ends. `batch` makes one step for all its calls.
- A request waits while the interface thread is busy: while a plug-in opens, a project loads, a
  render runs, or a system file window is open.
- **Speed.** A request is taken at the interface thread's next turn, so a call costs about one
  frame of the app. Measured with the app run without a window (debug
  build, 2026-11-12): 300 calls one after another on one connection took 3.5 ms each at the
  median for `rpc.ping` and `transport.locate` and 3.9 ms for `state.get` (95th percentile
  under 4 ms); `prowavectl call`, a new process for each call, 4.6 ms. With the window open
  the app draws at the display's rate, so expect up to one refresh (8 to 17 ms) a call; that
  has not been measured. `rpc.stats` gives the app's own figures.

## Limits

- **Bounce.** `export.bounce` is the app's bounce (File > Bounce > Project or Section): a
  second engine that has no sound card is handed a copy of the project by the code that
  hands the running engine the project, and its blocks are written to the file, faster than
  they would be played. What the app plays is in the file: audio regions (offsets, gains,
  fades, reverse, loops, Flex), instruments (built-in, Audio Unit, VST3), Aux strips, sends,
  VCA and group levels, Mute and Solo, pan, built-in and plug-in effects, automation, tempo
  changes, the Stereo Out with its effects and its fader. `render.range` is the same bounce
  with a fixed tail and a WAV file of 32-bit floats. The result's `not_rendered` names
  whatever should be in the file and is not; `complete` is true when it is empty. What can
  be in that list: an External MIDI track (its instrument is outside the app), an audio
  region whose file is missing, a plug-in that could not be opened, Singer phrases that
  have not been sung yet (a bounce does not sing: it uses what was sung).
  The metronome is never in a bounce. Like the app's output, a
  bounce is cut off at full scale (0 dBFS), also in a 32-bit float file. Plug-ins are opened
  again for the bounce, with the settings the running ones have: one that makes sound at
  random, or by the clock, gives another file each time. A bounce runs over the app's frames
  and on a thread of its own, so other calls are answered while it runs, and the app can go
  on playing; one bounce runs at a time. The files are WAV (16-bit, 24-bit, 32-bit float) or
  AIFF (16-bit, 24-bit) at the engine's sample rate; there is no FLAC or MP3 writer.
- **Plug-in windows** are the plug-ins' own windows: `plugin.window` opens and closes them,
  `ui.screenshot` does not show them.
- **A plug-in opens on the frame after it is put in a slot.** Its parameters are listed from
  then on: call `rpc.wait` first, or call `plugin.params` again. Parameters of Audio Units and
  VST3s are read and set on the running plug-in; a VST3's programs are not presets the app
  can list.
- **Parameters of the app's own instruments** (Channel Rack, 808, Singer) are listed where
  the app can automate them, but are set in their windows, not with `param.set`.
- **Not exposed as methods of their own:** Channel Rack steps, the Step Sequencer and Drummer
  editors, the Pitch Editor, Flex markers, take folders and comping, Vocal Top Line
  rendering, MIDI
  clock and time code ports, mix groups, track stacks (folders), Smart Tempo. What has a menu
  entry is reachable with `menu.invoke`; what is a window can be opened with `ui.window` and
  looked at with `ui.screenshot`.
- **Tracks cannot be reordered in the app by hand**; `track.move` does it with the project's
  own renumbering and refuses when the project has track stacks, and for folder and VCA
  tracks.
- **Device and buffer changes** happen only through `audio.set`.
- **Ids** are for one run of the app (see Naming things).
- A request line can be 8 MB at most; one call writes 200000 notes at most; a
  bounce is four hours at most.

## Without a window

`prowave --control-headless --socket /tmp/pw.sock` runs the app with no window, no menu bar
and (unless `--audio-output <device uid>` is given) no audio engine, serving the same methods
on that socket. It is for trying the interface and for tests: `ui.screenshot` there is drawn
by a software renderer, plug-ins are not opened without an engine, and it reads and writes no
recovery copies or recent-projects list.

## For maintainers

- The code is `crates/prowave-app/src/control_api/` (the app's side) and `crates/prowavectl/`
  (the client and the MCP server).
- Add a method in `m_core.rs`, `m_tracks.rs`, `m_regions.rs`, `m_ui.rs` or `render.rs` (the
  bounce calls): a
  function and an entry in that file's `methods()`. Then run the tests once with
  `PROWAVE_CONTROL_REGEN=1` to write this document's method table and
  `crates/prowavectl/methods.json` again. The test
  `the_reference_document_and_the_clients_list_match_the_registry` fails when either is stale.
- Build the client: `cargo build --release -p prowavectl`, then copy `target/release/prowavectl`
  to a folder on your PATH (`~/.local/bin`).
