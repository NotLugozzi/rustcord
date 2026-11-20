luna.log("Hello from " .. luna.plugin.name .. " " .. luna.plugin.version)
local greeting_count = luna.storage.get("greeting_count") or 0
greeting_count = greeting_count + 1
luna.storage.set("greeting_count", greeting_count)
luna.log("Greeting count: " .. tostring(greeting_count))
