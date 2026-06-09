--
-- Wireshark Lua dissector for the SteelSeries Arctis Nova 7 wireless dongle (HID).
--
-- Goal: make visible, in the Wireshark packet detail tree, exactly which bytes of
-- the device's HID reports we understand and which we do not. Decoding mirrors the
-- knowledge encoded in the Autoswapper backend (src-tauri/src/presence.rs); every
-- byte we cannot yet explain is surfaced under an "Unknown / undecoded" subtree and
-- flagged with an expert "note" so it shows up in Analyze -> Expert Information.
--
-- What we KNOW (confirmed by working parsers + unit tests in presence.rs):
--   0xB0 status report:  byte+3 = connection/charge state
--                                 (0x00 disconnected, 0x01 charging, 0x03 on battery)
--                        byte+2 = battery percent (startup/status snapshot)
--                        byte+4 = ChatMix "game" level, byte+5 = ChatMix "chat" level
--   0x45 wheel event:    byte+1 = ChatMix "game", byte+2 = ChatMix "chat"
--   0x52 mute event:     byte+2 = microphone muted (0x00 unmuted, 0x01 muted)
--   0xB9 power event:    byte+1 = connection state (0x02 off, 0x03 on)
--   0xB7 battery event:  byte+1 = battery percent
--
-- What we DON'T fully know (shown but marked uncertain / undecoded):
--   0xB0 byte+1 and byte+2 (battery %? charge flags? — values 0x01..0x03 and ~0x58/0x59
--   have been observed but are unconfirmed), and all trailing bytes of every report.
--
-- Install: drop this file in the Wireshark "Personal Lua Plugins" folder
--   (Help -> About Wireshark -> Folders), then Analyze -> Reload Lua Plugins (Ctrl+Shift+L).
--

local p_nova7 = Proto("nova7", "SteelSeries Arctis Nova 7 HID")

-- ---------------------------------------------------------------------------
-- Protocol constants
-- ---------------------------------------------------------------------------
local VID = 0x1038
local PIDS = { 0x2202, 0x2206, 0x220A, 0x223A, 0x2258, 0x22A1 }

local OP_STATUS  = 0xB0 -- battery / connection status (poll request + response, MI_03)
local OP_CHATMIX = 0x45 -- ChatMix wheel turned (unsolicited, MI_05)
local OP_MUTE    = 0x52 -- microphone mute toggled (unsolicited, MI_05)
local OP_CONN    = 0xB9 -- connection / power state CHANGE (unsolicited, MI_05)
local OP_BATTERY = 0xB7 -- battery level update (unsolicited, MI_05)

local OPCODE_NAMES = {
	[OP_STATUS]  = "Battery / connection status (poll)",
	[OP_CHATMIX] = "ChatMix wheel",
	[OP_MUTE]    = "Microphone mute",
	[OP_CONN]    = "Connection / power event",
	[OP_BATTERY] = "Battery level event",
}
local KNOWN_OPCODES = {
	[OP_STATUS] = true, [OP_CHATMIX] = true, [OP_MUTE] = true,
	[OP_CONN] = true, [OP_BATTERY] = true,
}

local CONN_NAMES = {
	[0x00] = "Disconnected / headset off",
	[0x01] = "Connected (charging)",
	[0x03] = "Connected (on battery)",
}
-- 0xB9 connection-event states (observed at headset power on/off).
local CONN_EVENT_NAMES = {
	[0x01] = "Connected (charging)",
	[0x02] = "Off / disconnected",
	[0x03] = "On / connected",
}
local MUTE_NAMES = { [0x00] = "Unmuted", [0x01] = "Muted" }

-- ---------------------------------------------------------------------------
-- Fields
-- ---------------------------------------------------------------------------
local f = p_nova7.fields
f.report_id  = ProtoField.uint8("nova7.report_id", "Report ID", base.HEX)
f.opcode     = ProtoField.uint8("nova7.opcode", "Opcode", base.HEX, OPCODE_NAMES)
f.connection = ProtoField.uint8("nova7.connection", "Connection state", base.HEX, CONN_NAMES)
f.cm_game    = ProtoField.uint8("nova7.chatmix.game", "ChatMix - Game", base.DEC)
f.cm_chat    = ProtoField.uint8("nova7.chatmix.chat", "ChatMix - Chat", base.DEC)
f.mic_muted  = ProtoField.uint8("nova7.mic_muted", "Microphone muted", base.HEX, MUTE_NAMES)
f.conn_event = ProtoField.uint8("nova7.connection_event", "Connection event", base.HEX, CONN_EVENT_NAMES)
f.battery    = ProtoField.uint8("nova7.battery_percent", "Battery percent", base.DEC)
f.uncertain  = ProtoField.uint8("nova7.uncertain", "Uncertain byte", base.HEX)
f.unknown    = ProtoField.bytes("nova7.unknown", "Undecoded bytes")
f.payload    = ProtoField.bytes("nova7.payload", "Raw payload")

-- ---------------------------------------------------------------------------
-- Expert info
-- ---------------------------------------------------------------------------
local e_uncertain = ProtoExpert.new("nova7.uncertain.expert",
	"Byte meaning unconfirmed (best guess only)", expert.group.UNDECODED, expert.severity.NOTE)
local e_unknown = ProtoExpert.new("nova7.unknown.expert",
	"Undecoded bytes - protocol not fully reverse-engineered", expert.group.UNDECODED, expert.severity.NOTE)
local e_unknown_op = ProtoExpert.new("nova7.opcode.unknown.expert",
	"Unknown opcode - no decoder for this report", expert.group.UNDECODED, expert.severity.WARN)
p_nova7.experts = { e_uncertain, e_unknown, e_unknown_op }

-- ---------------------------------------------------------------------------
-- Preferences
-- ---------------------------------------------------------------------------
p_nova7.prefs.heur = Pref.bool("Heuristic detection on USB transfers", true,
	"Try to recognise Nova 7 reports on any USB interrupt/control transfer, even when "
	.. "the capture did not include device enumeration (VID/PID).")

-- Direction of the USB transfer (1 = IN / device->host). Guarded so the plugin still
-- loads if the field name ever changes between Wireshark versions.
local f_dir
do
	local ok, fld = pcall(Field.new, "usb.endpoint_address.direction")
	if ok then f_dir = fld end
end

local f_usbhid_data
do
	local ok, fld = pcall(Field.new, "usbhid.data")
	if ok then f_usbhid_data = fld end
end

-- ---------------------------------------------------------------------------
-- Helpers
-- ---------------------------------------------------------------------------

-- The opcode sits at offset 0, or at offset 1 when a leading HID report-ID byte
-- (typically 0x00) is present. Return the offset of the opcode and its value.
local function find_opcode(tvb)
	local len = tvb:len()
	local limit = math.min(len, 2)
	for i = 0, limit - 1 do
		if KNOWN_OPCODES[tvb(i, 1):uint()] then
			return i, tvb(i, 1):uint()
		end
	end
	return nil, nil
end

-- Add a range as "uncertain" (shown, but meaning unconfirmed).
local function add_uncertain(tree, range, label)
	local item = tree:add(f.uncertain, range)
	item:append_text(" (" .. label .. ")")
	item:add_proto_expert_info(e_uncertain)
	return item
end

-- Add the trailing/unknown remainder as "undecoded".
local function add_unknown(tree, tvb, from)
	local len = tvb:len()
	if from < len then
		local item = tree:add(f.unknown, tvb(from, len - from))
		item:add_proto_expert_info(e_unknown)
	end
end

local function is_inbound()
	if f_dir == nil then return nil end
	local fi = f_dir()
	if fi == nil then return nil end
	return fi.value == 1
end

-- ---------------------------------------------------------------------------
-- Core dissection. Returns number of bytes consumed (0 = not ours).
-- ---------------------------------------------------------------------------
local function dissect(tvb, pinfo, tree)
	local len = tvb:len()
	if len == 0 then return 0 end

	local off, op = find_opcode(tvb)
	if op == nil then return 0 end

	pinfo.cols.protocol = "Nova7"
	local root = tree:add(p_nova7, tvb(), "SteelSeries Arctis Nova 7 HID")
	root:add(f.payload, tvb()):set_generated()

	if off > 0 then
		root:add(f.report_id, tvb(0, off))
	end
	root:add(f.opcode, tvb(off, 1))

	local inbound = is_inbound()
	local known = root:add(tvb(off, len - off), "Decoded (known)")

	if op == OP_STATUS then
		-- A host->device 0xB0 with an all-zero body is the periodic status poll request.
		if inbound == false then
			pinfo.cols.info = "Status poll request (0xB0)"
			known:append_text(": status poll request")
			add_unknown(root, tvb, off + 1)
			return len
		end

		if len > off + 1 then add_uncertain(known, tvb(off + 1, 1), "unconfirmed: charge flag?") end
		if len > off + 2 then known:add(f.battery, tvb(off + 2, 1)) end
		local info = "Status"
		if len > off + 3 then
			local v = tvb(off + 3, 1):uint()
			known:add(f.connection, tvb(off + 3, 1))
			info = "Status: " .. (CONN_NAMES[v] or string.format("unknown (0x%02X)", v))
		end
		if len > off + 4 then known:add(f.cm_game, tvb(off + 4, 1)) end
		if len > off + 5 then known:add(f.cm_chat, tvb(off + 5, 1)) end
		pinfo.cols.info = info
		add_unknown(root, tvb, off + 6)

	elseif op == OP_CHATMIX then
		if len > off + 1 then known:add(f.cm_game, tvb(off + 1, 1)) end
		if len > off + 2 then known:add(f.cm_chat, tvb(off + 2, 1)) end
		local g = (len > off + 1) and tvb(off + 1, 1):uint() or 0
		local c = (len > off + 2) and tvb(off + 2, 1):uint() or 0
		pinfo.cols.info = string.format("ChatMix wheel  game=%d chat=%d", g, c)
		add_unknown(root, tvb, off + 3)

	elseif op == OP_MUTE then
		if len > off + 1 then add_uncertain(known, tvb(off + 1, 1), "unconfirmed") end
		if len > off + 2 then
			local v = tvb(off + 2, 1):uint()
			known:add(f.mic_muted, tvb(off + 2, 1))
			pinfo.cols.info = "Microphone " .. (MUTE_NAMES[v] or string.format("? (0x%02X)", v))
		end
		add_unknown(root, tvb, off + 3)

	elseif op == OP_CONN then
		if len > off + 1 then
			local v = tvb(off + 1, 1):uint()
			known:add(f.conn_event, tvb(off + 1, 1))
			pinfo.cols.info = "Connection event: "
				.. (CONN_EVENT_NAMES[v] or string.format("unknown (0x%02X)", v))
		else
			pinfo.cols.info = "Connection event"
		end
		add_unknown(root, tvb, off + 2)

	elseif op == OP_BATTERY then
		if len > off + 1 then
			local pct = tvb(off + 1, 1):uint()
			known:add(f.battery, tvb(off + 1, 1))
			pinfo.cols.info = string.format("Battery event: %d%%", pct)
		else
			pinfo.cols.info = "Battery event"
		end
		add_unknown(root, tvb, off + 2)
	end

	return len
end

-- ---------------------------------------------------------------------------
-- Main dissector (used via usb.product table and "Decode As")
-- ---------------------------------------------------------------------------
function p_nova7.dissector(tvb, pinfo, tree)
	return dissect(tvb, pinfo, tree)
end

-- ---------------------------------------------------------------------------
-- Heuristic dissector (used when the capture lacks VID/PID enumeration)
-- ---------------------------------------------------------------------------
local function heuristic(tvb, pinfo, tree)
	if not p_nova7.prefs.heur then return false end
	return dissect(tvb, pinfo, tree) > 0
end

-- ---------------------------------------------------------------------------
-- Registration
-- ---------------------------------------------------------------------------
local usb_product = DissectorTable.get("usb.product")
for _, pid in ipairs(PIDS) do
	usb_product:add(VID * 0x10000 + pid, p_nova7)
end

p_nova7:register_heuristic("usb.interrupt", heuristic)
p_nova7:register_heuristic("usb.control", heuristic)

-- Some captures hand HID interrupt payloads to Wireshark's generic usbhid dissector
-- before USB heuristics get a chance to claim them. This post-dissector makes those
-- MI_05 reports visible too.
local p_nova7_post = Proto("nova7_usbhid", "SteelSeries Arctis Nova 7 HID post-dissector")
function p_nova7_post.dissector(tvb, pinfo, tree)
	if f_usbhid_data == nil then return end
	if tostring(pinfo.cols.protocol) == "Nova7" then return end

	local data = f_usbhid_data()
	if data == nil or data.range == nil then return end

	local hid_tvb = ByteArray.tvb(data.range:bytes(), "Nova 7 HID data")
	dissect(hid_tvb, pinfo, tree)
end
register_postdissector(p_nova7_post)
