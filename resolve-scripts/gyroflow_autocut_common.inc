local M = {}

local function shell_quote(path)
    if package.config:sub(1, 1) == "\\" then
        return '"' .. path:gsub('"', '""') .. '"'
    end
    return "'" .. path:gsub("'", "'\\''") .. "'"
end

local function ensure_directory(path)
    local command
    if package.config:sub(1, 1) == "\\" then
        command = "if not exist " .. shell_quote(path) .. " mkdir " .. shell_quote(path)
    else
        command = "mkdir -p " .. shell_quote(path)
    end
    pcall(os.execute, command)
end

local log_file = nil
do
    local base = os.getenv("LOCALAPPDATA")
    local log_directory
    local log_path
    if base then
        log_directory = base .. "\\NiYien\\GyroflowNiYien"
        log_path = log_directory .. "\\gyroflow-autocut.log"
    else
        local home = os.getenv("HOME")
        if home then
            log_directory = home .. "/Library/Application Support/NiYien/GyroflowNiYien"
            log_path = log_directory .. "/gyroflow-autocut.log"
        end
    end
    if log_directory then
        ensure_directory(log_directory)
        log_file = io.open(log_path, "ab")
    end
end

local function say(message)
    local line = "[Gyroflow Auto Cut] " .. message
    print(line)
    if log_file then
        log_file:write(os.date("!%Y-%m-%d %H:%M:%S ") .. line .. "\n")
        log_file:flush()
    end
end

local function items_of(values)
    local result = {}
    if values then
        for _, value in pairs(values) do
            local value_type = type(value)
            if value_type == "userdata" or value_type == "table" then
                result[#result + 1] = value
            end
        end
    end
    return result
end

local function object_id(item)
    local ok, value = pcall(function() return item:GetUniqueId() end)
    if ok and value then return tostring(value) end
    return tostring(item)
end

local function item_start(item)
    local ok, value = pcall(function() return item:GetStart() end)
    if ok then return tonumber(value) end
    return nil
end

local function find_track(timeline, media_type, item)
    local target_id = object_id(item)
    local track_count = tonumber(timeline:GetTrackCount(media_type)) or 0
    for track = 1, track_count do
        for _, candidate in ipairs(items_of(timeline:GetItemListInTrack(media_type, track))) do
            if object_id(candidate) == target_id then return track end
        end
    end
    return nil
end

local function read_trim_ranges_ms(path)
    local file = io.open(path, "rb")
    if not file then return nil, "cannot open project file" end
    local data = file:read("*a")
    file:close()

    local key_pos = data:find('"trim_ranges_ms"', 1, true)
    if not key_pos then return nil, "no trim_ranges_ms key" end
    local array_start = data:find("%[", key_pos)
    if not array_start then return nil, "malformed trim_ranges_ms" end

    local depth, index, array_end = 0, array_start, nil
    while index <= #data do
        local char = data:sub(index, index)
        if char == "[" then
            depth = depth + 1
        elseif char == "]" then
            depth = depth - 1
            if depth == 0 then
                array_end = index
                break
            end
        end
        index = index + 1
    end
    if not array_end then return nil, "unbalanced trim_ranges_ms" end

    local ranges = {}
    local body = data:sub(array_start + 1, array_end - 1)
    for first, last in body:gmatch("%[%s*([%d%.eE%+%-]+)%s*,%s*([%d%.eE%+%-]+)%s*%]") do
        local range_start, range_end = tonumber(first), tonumber(last)
        if range_start and range_end and (range_end < 0 or range_end > range_start) then
            ranges[#ranges + 1] = { range_start, range_end }
        end
    end
    table.sort(ranges, function(left, right) return left[1] < right[1] end)
    if #ranges == 0 then return nil, "trim_ranges_ms is empty" end
    return ranges
end

local function sibling_project_path(video_path)
    return video_path:gsub("%.[^%./\\]+$", "") .. ".gyroflow"
end

local function collect_audio_lanes(timeline, item, fallback_media_pool_item)
    local lanes = {}
    for _, linked_item in ipairs(items_of(item:GetLinkedItems())) do
        local track = find_track(timeline, "audio", linked_item)
        if track then
            local ok, media_pool_item = pcall(function() return linked_item:GetMediaPoolItem() end)
            lanes[#lanes + 1] = {
                item = linked_item,
                track = track,
                media_pool_item = (ok and media_pool_item) or fallback_media_pool_item,
            }
        end
    end
    table.sort(lanes, function(left, right)
        if left.track == right.track then
            return object_id(left.item) < object_id(right.item)
        end
        return left.track < right.track
    end)
    return lanes
end

local function build_segments(ranges, fps, frames, base, record_start)
    local segments = {}
    local record = record_start
    local last_source_frame = frames and (base + frames - 1) or nil
    for _, range in ipairs(ranges) do
        local range_end_ms = range[2]
        if range_end_ms < 0 and frames then
            range_end_ms = frames / fps * 1000.0 + range_end_ms
        end
        local source_start = base + math.floor(range[1] / 1000.0 * fps)
        local source_end = base + math.ceil(range_end_ms / 1000.0 * fps) - 1
        if source_start < base then source_start = base end
        if last_source_frame and source_end > last_source_frame then
            source_end = last_source_frame
        end
        if source_end >= source_start then
            local duration = source_end - source_start + 1
            segments[#segments + 1] = {
                source_start = source_start,
                source_end = source_end,
                record = record,
                duration = duration,
            }
            record = record + duration
        end
    end
    return segments
end

local function inspect_item(timeline, item)
    local media_pool_item = item:GetMediaPoolItem()
    if not media_pool_item then return nil, "has no media pool source" end

    local properties = media_pool_item:GetClipProperty()
    local name = (properties and properties["Clip Name"]) or "timeline item"
    local path = properties and properties["File Path"]
    if not path or path == "" then return nil, name .. ": no file path" end

    local ranges, range_error = read_trim_ranges_ms(sibling_project_path(path))
    if not ranges then return nil, name .. ": " .. range_error end

    local fps = tonumber(properties["FPS"])
    local frames = tonumber(properties["Frames"])
    if not fps or fps <= 0 then return nil, name .. ": no usable FPS" end

    local start_property = tonumber(properties["Start"])
    local end_property = tonumber(properties["End"])
    local base = 0
    if start_property and end_property and frames and start_property > 0
        and (end_property - start_property + 1) == frames then
        base = start_property
    end

    local record_start = item_start(item)
    if not record_start then return nil, name .. ": cannot read timeline position" end
    local video_track = find_track(timeline, "video", item)
    if not video_track then return nil, name .. ": cannot locate video track" end

    local segments = build_segments(ranges, fps, frames, base, record_start)
    if #segments == 0 then return nil, name .. ": ranges collapsed after clamping" end

    return {
        item = item,
        name = name,
        media_pool_item = media_pool_item,
        video_track = video_track,
        audio_lanes = collect_audio_lanes(timeline, item, media_pool_item),
        segments = segments,
    }
end

local function append_video(media_pool, model, segment, track)
    local result = items_of(media_pool:AppendToTimeline({ {
        mediaPoolItem = model.media_pool_item,
        startFrame = segment.source_start,
        endFrame = segment.source_end,
        trackIndex = track,
        recordFrame = segment.record,
        mediaType = 1,
    } }))
    if #result ~= 1 then return nil, result end
    return result[1], result
end

local function append_audio(media_pool, lane, segment, track)
    return items_of(media_pool:AppendToTimeline({ {
        mediaPoolItem = lane.media_pool_item,
        startFrame = segment.source_start,
        endFrame = segment.source_end,
        trackIndex = track,
        recordFrame = segment.record,
        mediaType = 2,
    } }))
end

local function set_linked(timeline, video, audio_items)
    if #audio_items == 0 then return true end
    local group = { video }
    for _, audio in ipairs(audio_items) do group[#group + 1] = audio end
    local ok, linked = pcall(function() return timeline:SetClipsLinked(group, true) end)
    return ok and linked == true
end

local function validate_linked_audio(video, expected_audio)
    if #expected_audio == 0 then return true end
    local linked_ids = {}
    for _, linked in ipairs(items_of(video:GetLinkedItems())) do
        linked_ids[object_id(linked)] = true
    end
    for _, audio in ipairs(expected_audio) do
        if not linked_ids[object_id(audio)] then return false end
    end
    return true
end

local function delete_items(timeline, items)
    if #items == 0 then return true end
    local ok, deleted = pcall(function() return timeline:DeleteClips(items, false) end)
    return ok and deleted == true
end

local function delete_track(timeline, media_type, track)
    local ok, deleted = pcall(function() return timeline:DeleteTrack(media_type, track) end)
    return ok and deleted == true
end

local function cleanup_tracks(timeline, temporary, items)
    if items and #items > 0 then delete_items(timeline, items) end
    for index = #temporary.audio_tracks, 1, -1 do
        delete_track(timeline, "audio", temporary.audio_tracks[index])
    end
    delete_track(timeline, "video", temporary.video_track)
end

local function create_temporary_tracks(timeline, audio_lane_count)
    local temporary = { audio_tracks = {} }
    temporary.video_track = (tonumber(timeline:GetTrackCount("video")) or 0) + 1
    if not timeline:AddTrack("video") then return nil, "could not add a temporary video track" end

    for _ = 1, audio_lane_count do
        local track = (tonumber(timeline:GetTrackCount("audio")) or 0) + 1
        if not timeline:AddTrack("audio") then
            cleanup_tracks(timeline, temporary, {})
            return nil, "could not add a temporary audio track"
        end
        temporary.audio_tracks[#temporary.audio_tracks + 1] = track
    end
    return temporary
end

local function append_segment_group(media_pool, timeline, model, segment, video_track, audio_tracks)
    local video, video_items = append_video(media_pool, model, segment, video_track)
    if not video then return nil, video_items, "video append failed" end

    local created = { video }
    local audio_items = {}
    for lane_index, lane in ipairs(model.audio_lanes) do
        local appended = append_audio(media_pool, lane, segment, audio_tracks[lane_index])
        if #appended == 0 then
            return nil, created, string.format("audio append failed for lane %d", lane_index)
        end
        for _, audio in ipairs(appended) do
            audio_items[#audio_items + 1] = audio
            created[#created + 1] = audio
        end
    end

    if not set_linked(timeline, video, audio_items) then
        return nil, created, "could not link rebuilt video and audio"
    end
    if not validate_linked_audio(video, audio_items) then
        return nil, created, "rebuilt audio did not remain linked"
    end
    return { video = video, audio = audio_items }, created
end

local function append_all_segments(media_pool, timeline, model, video_track, audio_tracks)
    local groups, created = {}, {}
    for _, segment in ipairs(model.segments) do
        local group, partial, append_error = append_segment_group(
            media_pool,
            timeline,
            model,
            segment,
            video_track,
            audio_tracks
        )
        for _, item in ipairs(partial or {}) do created[#created + 1] = item end
        if not group then return nil, created, append_error end
        groups[#groups + 1] = group
    end
    return groups, created
end

local function copy_stage_grades(original, groups)
    local targets = {}
    for _, group in ipairs(groups) do targets[#targets + 1] = group.video end
    local ok, copied = pcall(function() return original:CopyGrades(targets) end)
    return ok and copied == true
end

local function copy_final_grades(staging_groups, final_groups)
    if #staging_groups ~= #final_groups then return false end
    for index, final_group in ipairs(final_groups) do
        local ok, copied = pcall(function()
            return staging_groups[index].video:CopyGrades({ final_group.video })
        end)
        if not ok or copied ~= true then return false end
    end
    return true
end

local function process_item(media_pool, timeline, item)
    local model, inspect_error = inspect_item(timeline, item)
    if not model then
        say("skip " .. inspect_error)
        return "skipped"
    end

    local temporary, temporary_error = create_temporary_tracks(timeline, #model.audio_lanes)
    if not temporary then
        say(model.name .. ": " .. temporary_error .. " - original kept")
        return "skipped"
    end

    local staging_groups, staging_items, staging_error = append_all_segments(
        media_pool,
        timeline,
        model,
        temporary.video_track,
        temporary.audio_tracks
    )
    if not staging_groups then
        say(model.name .. ": staging failed (" .. staging_error .. ") - original kept")
        cleanup_tracks(timeline, temporary, staging_items)
        return "skipped"
    end
    if not copy_stage_grades(model.item, staging_groups) then
        say(model.name .. ": grade copy failed - original kept. Attach Gyroflow as a Color page node; Edit-page effects cannot be copied by the scripting API.")
        cleanup_tracks(timeline, temporary, staging_items)
        return "skipped"
    end

    local originals = { model.item }
    for _, lane in ipairs(model.audio_lanes) do originals[#originals + 1] = lane.item end
    if not delete_items(timeline, originals) then
        say(string.format(
            "%s: original deletion was not confirmed - complete graded AV staging remains on video track %d",
            model.name,
            temporary.video_track
        ))
        return "recovery"
    end

    local final_audio_tracks = {}
    for _, lane in ipairs(model.audio_lanes) do
        final_audio_tracks[#final_audio_tracks + 1] = lane.track
    end
    local final_groups, final_items, final_error = append_all_segments(
        media_pool,
        timeline,
        model,
        model.video_track,
        final_audio_tracks
    )
    if not final_groups then
        delete_items(timeline, final_items)
        say(string.format(
            "%s: final rebuild failed (%s) - complete graded AV staging remains on video track %d",
            model.name,
            final_error,
            temporary.video_track
        ))
        return "recovery"
    end
    if not copy_final_grades(staging_groups, final_groups) then
        delete_items(timeline, final_items)
        say(string.format(
            "%s: final grade copy failed - complete graded AV staging remains on video track %d",
            model.name,
            temporary.video_track
        ))
        return "recovery"
    end

    for _, group in ipairs(final_groups) do
        if not validate_linked_audio(group.video, group.audio) then
            delete_items(timeline, final_items)
            say(string.format(
                "%s: final audio validation failed - complete graded AV staging remains on video track %d",
                model.name,
                temporary.video_track
            ))
            return "recovery"
        end
    end

    cleanup_tracks(timeline, temporary, staging_items)
    for index, segment in ipairs(model.segments) do
        say(string.format(
            "%s segment %d: source %d..%d -> timeline frame %d (video track %d, audio lanes %d)",
            model.name,
            index,
            segment.source_start,
            segment.source_end,
            segment.record,
            model.video_track,
            #model.audio_lanes
        ))
    end
    say(string.format(
        "%s: replaced with %d linked AV segment(s) on video track %d",
        model.name,
        #model.segments,
        model.video_track
    ))
    return "completed"
end

local function current_track_items(timeline, anchor)
    local track = find_track(timeline, "video", anchor)
    if not track then return nil, "could not locate the playhead item's video track" end
    local items = items_of(timeline:GetItemListInTrack("video", track))
    table.sort(items, function(left, right)
        local left_start = item_start(left) or math.huge
        local right_start = item_start(right) or math.huge
        if left_start == right_start then return object_id(left) < object_id(right) end
        return left_start < right_start
    end)
    return items, nil, track
end

function M.run(mode, resolve_app)
    if mode ~= "clip" and mode ~= "track" then
        say("unknown mode: " .. tostring(mode))
        return false
    end

    resolve_app = resolve_app or resolve or (bmd and bmd.scriptapp and bmd.scriptapp("Resolve"))
    if not resolve_app then
        say("cannot acquire the Resolve scripting object")
        return false
    end
    local project_manager = resolve_app:GetProjectManager()
    local project = project_manager and project_manager:GetCurrentProject()
    if not project then
        say("no current project")
        return false
    end
    local timeline = project:GetCurrentTimeline()
    if not timeline then
        say("no current timeline")
        return false
    end
    local anchor = timeline:GetCurrentVideoItem()
    if not anchor then
        say("the playhead is not over a video item")
        return false
    end

    local targets, target_error, track
    if mode == "clip" then
        targets = { anchor }
    else
        targets, target_error, track = current_track_items(timeline, anchor)
        if not targets then
            say(target_error)
            return false
        end
        say(string.format("Current Track: scanning %d item(s) on video track %d", #targets, track))
    end

    local media_pool = project:GetMediaPool()
    local completed, skipped = 0, 0
    for _, item in ipairs(targets) do
        local status = process_item(media_pool, timeline, item)
        if status == "completed" then
            completed = completed + 1
        elseif status == "recovery" then
            say(string.format(
                "%s stopped after a recovery copy was retained (%d completed, %d skipped)",
                mode == "clip" and "Current Clip" or "Current Track",
                completed,
                skipped
            ))
            return false
        else
            skipped = skipped + 1
        end
    end

    say(string.format(
        "%s finished: %d completed, %d skipped",
        mode == "clip" and "Current Clip" or "Current Track",
        completed,
        skipped
    ))
    return completed > 0
end

return M
