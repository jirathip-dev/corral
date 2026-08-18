#!/usr/bin/env ruby
# asc-beta-state.rb — TestFlight beta state for com.corral.fleetnotifier.
# Uses Spaceship to mint a valid ASC token, then raw REST for the fields
# Spaceship's model objects don't expose (state, inviteType, isInternalGroup).
# Run with homebrew ruby: /opt/homebrew/opt/ruby/bin/ruby scripts/asc-beta-state.rb
# Reads credentials from fastlane/.env (same as the TestFlight lane).
require "fastlane"
require "spaceship"
require "net/http"
require "uri"
require "json"

FastlaneCore::UI.verbose(false)
here = File.expand_path(__dir__)
env = {}
File.readlines(File.join(here, "../fastlane/.env")).each do |l|
  k, v = l.strip.split("=", 2)
  env[k] = v if k && v
end

token_obj = Spaceship::ConnectAPI::Token.create(
  key_id: env.fetch("ASC_KEY_ID"),
  issuer_id: env.fetch("ASC_ISSUER_ID"),
  filepath: File.expand_path(File.basename(env.fetch("ASC_KEY_PATH")), File.join(here, "../fastlane"))
)
bearer = token_obj.text

def api_get(path, bearer)
  uri = URI("https://api.appstoreconnect.apple.com#{path}")
  req = Net::HTTP::Get.new(uri)
  req["Authorization"] = "Bearer #{bearer}"
  res = Net::HTTP.start(uri.host, uri.port, use_ssl: true) { |h| h.request(req) }
  { code: res.code.to_i, body: (JSON.parse(res.body) rescue { "raw" => res.body }) }
end

APP_ID = "6802181286"

bg = api_get("/v1/betaGroups?filter[app]=#{APP_ID}&limit=20", bearer)
puts "=== betaGroups (http #{bg[:code]}) ==="
(bg[:body]["data"] || []).each do |g|
  a = g["attributes"] || {}
  puts "  id=#{g["id"]}"
  puts "    name=#{a["name"].inspect} isInternalGroup=#{a["isInternalGroup"].inspect}"
  puts "    publicLinkEnabled=#{a["publicLinkEnabled"].inspect} publicLink=#{a["publicLink"].inspect}"
  puts "    hasAccessToAllBuilds=#{a["hasAccessToAllBuilds"].inspect} created=#{a["createdDate"]}"
  # which builds is this group entitled to?
  b = api_get("/v1/betaGroups/#{g["id"]}/builds?limit=10", bearer)
  ids = (b[:body]["data"] || []).map { |x| x["id"] }
  puts "    builds attached: #{ids.count} #{ids.inspect}"
  t = api_get("/v1/betaGroups/#{g["id"]}/betaTesters?limit=20", bearer)
  puts "    testers in group: #{(t[:body]["data"] || []).count}"
  (t[:body]["data"] || []).each do |x|
    xa = x["attributes"] || {}
    puts "      #{xa["email"].inspect} state=#{xa["state"].inspect} inviteType=#{xa["inviteType"].inspect}"
  end
end

bt = api_get("/v1/betaTesters?filter[apps]=#{APP_ID}&limit=20", bearer)
puts "\n=== betaTesters on app (http #{bt[:code]}) ==="
(bt[:body]["data"] || []).each do |t|
  a = t["attributes"] || {}
  puts "  id=#{t["id"]} email=#{a["email"].inspect} state=#{a["state"].inspect} inviteType=#{a["inviteType"].inspect}"
end

bd = api_get("/v1/builds?filter[app]=#{APP_ID}&limit=5", bearer)
puts "\n=== builds (http #{bd[:code]}) ==="
(bd[:body]["data"] || []).each do |b|
  a = b["attributes"] || {}
  puts "  id=#{b["id"]} version=#{a["version"]} state=#{a["processingState"]} expired=#{a["expired"]}"
  # betaAppReviewSubmission + build beta detail (external review state)
  d = api_get("/v1/builds/#{b["id"]}/buildBetaDetail", bearer)
  da = d[:body].dig("data", "attributes") || {}
  puts "    internalBuildState=#{da["internalBuildState"].inspect} externalBuildState=#{da["externalBuildState"].inspect}"
end

# Does the app have the export-compliance / beta license agreement blockers?
ba = api_get("/v1/apps/#{APP_ID}/betaAppReviewDetail", bearer)
puts "\n=== betaAppReviewDetail (http #{ba[:code]}) ==="
puts "  #{(ba[:body].dig("data", "attributes") || {}).inspect}"

bl = api_get("/v1/apps/#{APP_ID}/betaLicenseAgreement", bearer)
puts "\n=== betaLicenseAgreement (http #{bl[:code]}) ==="
puts "  #{(bl[:body].dig("data", "attributes") || {}).to_s[0..200]}"
