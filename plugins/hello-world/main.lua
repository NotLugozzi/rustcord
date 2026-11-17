luna.log("Hello from " .. luna.plugin.name .. " " .. luna.plugin.version)
luna.storage.set("greeting_count", 1)
luna.log("Greeting count: " .. tostring(luna.storage.get("greeting_count")))
