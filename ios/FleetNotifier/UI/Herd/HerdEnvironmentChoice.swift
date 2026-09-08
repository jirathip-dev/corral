import CoreLocation
import SwiftUI

// Stored independently of Catppuccin. Location is an optional, already-granted
// cached input: this feature never asks permission or starts location updates.
enum HerdEnvironmentChoice: String, CaseIterable { case auto = "Auto", day = "Day", night = "Night" }
struct HerdLocationSample {
    let latitude: Double
    let longitude: Double
    let accuracy: Double
    let timestamp: Date
    let authorized: Bool
    func usable(at now: Date) -> Bool {
        authorized && latitude.isFinite && longitude.isFinite && accuracy.isFinite
            && abs(latitude) <= 90 && abs(longitude) <= 180 && accuracy >= 0 && accuracy <= 10_000
            && now.timeIntervalSince(timestamp) >= 0 && now.timeIntervalSince(timestamp) <= 86_400
    }
}
struct HerdLighting: Equatable {
    let night: Bool
    let explanation: String
    let nextChange: Date
}

enum HerdSun {
    static func resolve(_ choice: HerdEnvironmentChoice, now: Date, calendar: Calendar = .current,
                        location: HerdLocationSample? = nil) -> HerdLighting {
        if choice != .auto {
            return HerdLighting(night:choice == .night,explanation:choice.rawValue,nextChange:.distantFuture)
        }
        let midnight = calendar.startOfDay(for: now)
        let tomorrow = calendar.date(byAdding:.day,value:1,to:midnight) ?? now.addingTimeInterval(86_400)
        let fixedRise = calendar.date(bySettingHour:7,minute:0,second:0,of:now) ?? midnight.addingTimeInterval(25_200)
        let fixedSet = calendar.date(bySettingHour:19,minute:0,second:0,of:now) ?? midnight.addingTimeInterval(68_400)
        var rise = fixedRise, set = fixedSet, solar = false
        if let location, location.usable(at:now),
           let sunrise = event(now,calendar:calendar,location:location,rise:true),
           let sunset = event(now,calendar:calendar,location:location,rise:false), sunrise < sunset {
            rise = sunrise; set = sunset; solar = true
        }
        let next = [rise,set,tomorrow].filter { $0 > now }.min() ?? tomorrow
        return HerdLighting(night:now < rise || now >= set,
                            explanation:solar ? "Auto · local sunrise/sunset" : "Auto · local clock 07:00–19:00",
                            nextChange:next)
    }
    // NOAA sunrise equation, civil day input and standard 90.833° zenith.
    // Polar no-rise/no-set and invalid results explicitly take the fixed clock.
    static func event(_ date: Date, calendar: Calendar, location: HerdLocationSample, rise: Bool) -> Date? {
        var civil = Calendar(identifier:.gregorian)
        civil.timeZone = calendar.timeZone
        guard let ordinal = civil.ordinality(of:.day,in:.year,for:date) else { return nil }
        let radians = Double.pi/180
        let lngHour = location.longitude/15
        let t = Double(ordinal) + ((rise ? 6 : 18)-lngHour)/24
        let mean = 0.9856*t-3.289
        let longitude = normalized(mean+1.916*sin(mean*radians)+0.020*sin(2*mean*radians)+282.634)
        var rightAscension = normalized(atan(0.91764*tan(longitude*radians))/radians)
        rightAscension += floor(longitude/90)*90-floor(rightAscension/90)*90
        rightAscension /= 15
        let sinDeclination = 0.39782*sin(longitude*radians)
        let cosDeclination = cos(asin(sinDeclination))
        let cosHour = (cos(90.833*radians)-sinDeclination*sin(location.latitude*radians))
            / (cosDeclination*cos(location.latitude*radians))
        guard cosHour.isFinite, abs(cosHour) <= 1 else { return nil }
        let hour = (rise ? 360-acos(cosHour)/radians : acos(cosHour)/radians)/15
        let utcHour = (hour+rightAscension-0.06571*t-6.622-lngHour).truncatingRemainder(dividingBy:24)
        var utc = Calendar(identifier:.gregorian)
        utc.timeZone = TimeZone(secondsFromGMT:0) ?? .gmt
        let components = civil.dateComponents([.year,.month,.day],from:date)
        guard let day = utc.date(from:components) else { return nil }
        let candidate = day.addingTimeInterval((utcHour < 0 ? utcHour+24 : utcHour)*3600)
        return [-86_400.0,0,86_400].map { candidate.addingTimeInterval($0) }
            .first { calendar.isDate($0,inSameDayAs:date) }
    }
    static func normalized(_ degrees: Double) -> Double {
        let n = degrees.truncatingRemainder(dividingBy:360)
        return n < 0 ? n+360 : n
    }
}

@MainActor
final class HerdLocation: NSObject, ObservableObject, CLLocationManagerDelegate {
    @Published private(set) var revision = 0
    private let manager = CLLocationManager()
    override init() {
        super.init()
        manager.delegate = self
    }
    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) { revision += 1 }
    func sample() -> HerdLocationSample? {
        let granted = manager.authorizationStatus == .authorizedAlways || manager.authorizationStatus == .authorizedWhenInUse
        guard granted, let cached = manager.location else { return nil }
        return HerdLocationSample(latitude:cached.coordinate.latitude,longitude:cached.coordinate.longitude,
                                  accuracy:cached.horizontalAccuracy,timestamp:cached.timestamp,authorized:granted)
    }
}

struct HerdEnvironmentSettings: View {
    @AppStorage("herdEnvironment") private var choice = HerdEnvironmentChoice.auto
    var body: some View {
        Section("Herd environment") {
            Picker("Ranch light",selection:$choice) {
                ForEach(HerdEnvironmentChoice.allCases,id:\.self) { Text($0.rawValue).tag($0) }
            }.frame(minHeight:44)
            Text("Auto uses an already-authorized cached location for sunrise and sunset. Otherwise, Day is 07:00–19:00 on this device. Location is never required.")
                .font(.caption)
        }
    }
}
