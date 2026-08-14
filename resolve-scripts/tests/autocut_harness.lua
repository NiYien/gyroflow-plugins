local function script_dir()
    local source = debug.getinfo(1, "S").source or ""
    if source:sub(1, 1) == "@" then source = source:sub(2) end
    return source:match("^(.*)[/\\][^/\\]+$") or "."
end

local expected_log_dir = "C:\\Users\\Autocut Test\\NiYien\\GyroflowNiYien"
local expected_log_path = expected_log_dir .. "\\gyroflow-autocut.log"
local log_events = {}
local original_getenv = os.getenv
local original_execute = os.execute
local original_open = io.open

os.getenv = function(name)
    if name == "LOCALAPPDATA" then return "C:\\Users\\Autocut Test" end
    if name == "HOME" then return nil end
    return original_getenv(name)
end
os.execute = function(command)
    log_events[#log_events + 1] = { kind = "mkdir", value = command }
    return 0
end
io.open = function(path, mode)
    if mode == "ab" then
        log_events[#log_events + 1] = { kind = "open", value = path }
        return {
            write = function() end,
            flush = function() end,
        }
    end
    return original_open(path, mode)
end

local common = dofile(script_dir() .. "/../gyroflow_autocut_common.inc")

os.getenv = original_getenv
os.execute = original_execute
io.open = original_open

assert(log_events[1] and log_events[1].kind == "mkdir", "log directory must be created before opening the file")
assert(log_events[1].value:find(expected_log_dir, 1, true), "log directory creation uses the wrong path")
assert(log_events[2] and log_events[2].kind == "open", "log file was not opened after creating its directory")
assert(log_events[2].value == expected_log_path, "log file uses the wrong path")

local function check(condition, message)
    if not condition then error(message, 2) end
end

local next_id = 0
local function new_item(kind, media_pool_item, record_start)
    next_id = next_id + 1
    local item = {
        kind = kind,
        media_pool_item = media_pool_item,
        record_start = record_start,
        id = tostring(next_id),
        linked = {},
        grade = nil,
    }
    function item:GetUniqueId() return self.id end
    function item:GetMediaPoolItem() return self.media_pool_item end
    function item:GetStart() return self.record_start end
    function item:GetLinkedItems() return self.linked end
    function item:CopyGrades(targets)
        for _, target in ipairs(targets) do target.grade = self.grade end
        return true
    end
    return item
end

local function link_group(items)
    for _, item in ipairs(items) do
        item.linked = {}
        for _, other in ipairs(items) do
            if other ~= item then item.linked[#item.linked + 1] = other end
        end
    end
end

local function remove_item(items, target)
    for index = #items, 1, -1 do
        if items[index] == target then table.remove(items, index) end
    end
end

local function write_fixture(ranges)
    local base = os.tmpname()
    local video_path = base .. ".mp4"
    local project_path = base .. ".gyroflow"
    local file = assert(io.open(project_path, "wb"))
    file:write('{"trim_ranges_ms":' .. ranges .. '}')
    file:close()
    return video_path, project_path
end

local function new_media_pool_item(name, path)
    local media_pool_item = {}
    function media_pool_item:GetClipProperty()
        return {
            ["Clip Name"] = name,
            ["File Path"] = path,
            ["FPS"] = "10",
            ["Frames"] = "1000",
            ["Start"] = "0",
            ["End"] = "999",
        }
    end
    return media_pool_item
end

local function new_fixture(failure_mode, item_count, ranges)
    local timeline = {
        tracks = { video = { {} }, audio = { {} } },
        failure_mode = failure_mode,
        deleted_originals = 0,
    }
    local media_pool = { timeline = timeline }
    local cleanup_paths = {}

    function timeline:GetTrackCount(kind) return #self.tracks[kind] end
    function timeline:GetItemListInTrack(kind, track) return self.tracks[kind][track] end
    function timeline:GetCurrentVideoItem() return self.current_item end
    function timeline:AddTrack(kind)
        self.tracks[kind][#self.tracks[kind] + 1] = {}
        return true
    end
    function timeline:DeleteTrack(kind, track)
        if not self.tracks[kind][track] then return false end
        table.remove(self.tracks[kind], track)
        return true
    end
    function timeline:DeleteClips(items)
        for _, target in ipairs(items) do
            for _, kind in ipairs({ "video", "audio" }) do
                for _, track in ipairs(self.tracks[kind]) do remove_item(track, target) end
            end
            if target.original then self.deleted_originals = self.deleted_originals + 1 end
        end
        return true
    end
    function timeline:SetClipsLinked(items, linked)
        if not linked then return false end
        link_group(items)
        return true
    end

    function media_pool:AppendToTimeline(infos)
        local info = infos[1]
        local kind = info.mediaType == 1 and "video" or "audio"
        local original_count = 1
        if self.timeline.failure_mode == "stage_audio"
            and kind == "audio" and info.trackIndex > original_count then
            return {}
        end
        if self.timeline.failure_mode == "final_audio"
            and kind == "audio" and info.trackIndex <= original_count then
            return {}
        end
        local item = new_item(kind, info.mediaPoolItem, info.recordFrame)
        item.source_start = info.startFrame
        item.source_end = info.endFrame
        table.insert(self.timeline.tracks[kind][info.trackIndex], item)
        return { item }
    end

    local project = {}
    function project:GetCurrentTimeline() return timeline end
    function project:GetMediaPool() return media_pool end
    local project_manager = {}
    function project_manager:GetCurrentProject() return project end
    local resolve_app = {}
    function resolve_app:GetProjectManager() return project_manager end

    for index = 1, item_count do
        local video_path, project_path = write_fixture(ranges or '[[1000,2000],[3000,3500]]')
        cleanup_paths[#cleanup_paths + 1] = project_path
        local source = new_media_pool_item("clip-" .. index, video_path)
        local video = new_item("video", source, index * 100)
        local audio = new_item("audio", source, index * 100)
        video.original = true
        audio.original = true
        video.grade = "gyroflow-color-node"
        link_group({ video, audio })
        table.insert(timeline.tracks.video[1], video)
        table.insert(timeline.tracks.audio[1], audio)
        if index == 1 then timeline.current_item = video end
    end

    return {
        resolve_app = resolve_app,
        timeline = timeline,
        cleanup = function()
            for _, path in ipairs(cleanup_paths) do os.remove(path) end
        end,
    }
end

local function assert_linked_pairs(timeline, track, expected)
    check(#timeline.tracks.video[track] == expected, "unexpected video segment count")
    check(#timeline.tracks.audio[track] == expected, "unexpected audio segment count")
    for index, video in ipairs(timeline.tracks.video[track]) do
        local audio = timeline.tracks.audio[track][index]
        check(#video.linked == 1 and video.linked[1] == audio, "video is not linked to expected audio")
        check(#audio.linked == 1 and audio.linked[1] == video, "audio is not linked to expected video")
        check(video.grade == "gyroflow-color-node", "grade was not preserved")
    end
end

local success = new_fixture(nil, 2)
check(common.run("track", success.resolve_app) == true, "track mode should succeed")
check(#success.timeline.tracks.video == 1, "temporary video track was not removed")
check(#success.timeline.tracks.audio == 1, "temporary audio track was not removed")
assert_linked_pairs(success.timeline, 1, 4)
success.cleanup()

local negative_end = new_fixture(nil, 1, '[[1000,-1000]]')
check(common.run("clip", negative_end.resolve_app) == true, "negative range end should resolve from media duration")
assert_linked_pairs(negative_end.timeline, 1, 1)
check(negative_end.timeline.tracks.video[1][1].source_start == 10, "negative range start frame is incorrect")
check(negative_end.timeline.tracks.video[1][1].source_end == 989, "negative range end frame is incorrect")
negative_end.cleanup()

local stage_failure = new_fixture("stage_audio", 1)
check(common.run("clip", stage_failure.resolve_app) == false, "stage audio failure should not report success")
check(stage_failure.timeline.deleted_originals == 0, "original AV was deleted after staging failure")
check(#stage_failure.timeline.tracks.video == 1, "failed staging video track was not removed")
check(#stage_failure.timeline.tracks.audio == 1, "failed staging audio track was not removed")
assert_linked_pairs(stage_failure.timeline, 1, 1)
stage_failure.cleanup()

local final_failure = new_fixture("final_audio", 1)
check(common.run("clip", final_failure.resolve_app) == false, "final audio failure should retain recovery staging")
check(final_failure.timeline.deleted_originals == 2, "original AV was not removed before final failure")
check(#final_failure.timeline.tracks.video == 2, "recovery video track is missing")
check(#final_failure.timeline.tracks.audio == 2, "recovery audio track is missing")
assert_linked_pairs(final_failure.timeline, 2, 2)
check(#final_failure.timeline.tracks.video[1] == 0, "partial final video was not removed")
check(#final_failure.timeline.tracks.audio[1] == 0, "partial final audio remains")
final_failure.cleanup()

print("Resolve auto-cut fake timeline harness passed.")
