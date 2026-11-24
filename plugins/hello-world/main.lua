local plugin = {}

function plugin.init()
  luna.log("Hello from " .. luna.plugin.name .. " " .. luna.plugin.version)
  luna.on("application.started", function(event)
    local greeting_count = (luna.storage.get("greeting_count") or 0) + 1
    luna.storage.set("greeting_count", greeting_count)
    luna.log("Rustcord has started! Greeting count: " .. tostring(greeting_count))
  end)
end

function plugin.shutdown()
  luna.log("Goodbye from " .. luna.plugin.name)
end

return plugin
