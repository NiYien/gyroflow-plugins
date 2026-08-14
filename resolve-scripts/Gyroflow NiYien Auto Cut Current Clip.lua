local function script_dir()
    local source = debug.getinfo(1, "S").source or ""
    if source:sub(1, 1) == "@" then source = source:sub(2) end
    return source:match("^(.*)[/\\][^/\\]+$") or "."
end

local ok, common = pcall(dofile, script_dir() .. "/gyroflow_autocut_common.inc")
if not ok then
    print("[Gyroflow Auto Cut] failed to load shared module: " .. tostring(common))
    return
end

return common.run("clip")
