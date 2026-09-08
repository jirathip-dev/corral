import XCTest
@testable import FleetNotifier

final class HerdEnvironmentTests: XCTestCase {
    private func date(_ iso:String) throws -> Date { try XCTUnwrap(ISO8601DateFormatter().date(from:iso)) }
    private var utc: Calendar {
        var value = Calendar(identifier:.gregorian)
        value.timeZone = .gmt
        return value
    }
    func testManualAndAutoFixedBoundaryClockCases() throws {
        for (instant,night) in [("2026-09-08T06:59:59Z",true),("2026-09-08T07:00:00Z",false),
                                ("2026-09-08T18:59:59Z",false),("2026-09-08T19:00:00Z",true),
                                ("2026-09-08T23:59:59Z",true),("2026-09-09T00:00:00Z",true)] {
            let now = try date(instant)
            let auto = HerdSun.resolve(.auto,now:now,calendar:utc)
            XCTAssertEqual(auto.night,night,instant)
            XCTAssertGreaterThan(auto.nextChange,now)
            XCTAssertTrue(auto.explanation.contains("local clock"))
            XCTAssertFalse(HerdSun.resolve(.day,now:now,calendar:utc).night)
            XCTAssertTrue(HerdSun.resolve(.night,now:now,calendar:utc).night)
        }
        var local = utc
        local.timeZone = try XCTUnwrap(TimeZone(identifier:"Asia/Bangkok"))
        XCTAssertFalse(HerdSun.resolve(.auto,now:try date("2026-09-08T00:00:00Z"),calendar:local).night)
        XCTAssertTrue(HerdSun.resolve(.auto,now:try date("2026-09-08T12:00:00Z"),calendar:local).night)
        local.timeZone = try XCTUnwrap(TimeZone(identifier:"America/New_York"))
        XCTAssertFalse(HerdSun.resolve(.auto,now:try date("2026-03-08T11:00:00Z"),calendar:local).night)
    }
    func testDeniedInvalidExpiredFutureAndPolarSamplesFallBack() throws {
        let now = try date("2026-06-21T12:00:00Z")
        for (latitude,longitude,accuracy,age,authorized) in [(51.0,0.0,10.0,0.0,false),
            (.nan,0,10,0,true),(0,.infinity,10,0,true),(91,0,10,0,true),
            (0,181,10,0,true),(0,0,-1,0,true),(0,0,10001,0,true),
            (0,0,10,86401,true),(0,0,10,-1,true),(89,0,10,0,true)] {
            let sample = HerdLocationSample(latitude:latitude,longitude:longitude,accuracy:accuracy,
                timestamp:now.addingTimeInterval(-age),authorized:authorized)
            let resolved = HerdSun.resolve(.auto,now:now,calendar:utc,location:sample)
            XCTAssertEqual(resolved,HerdSun.resolve(.auto,now:now,calendar:utc))
        }
    }
    func testSolarDateDoesNotDependOnDeviceCalendarEra() throws {
        let now = try date("2026-06-21T12:00:00Z")
        let sample = HerdLocationSample(latitude:51.48,longitude:0,accuracy:20,timestamp:now,authorized:true)
        var buddhist = Calendar(identifier:.buddhist)
        buddhist.timeZone = .gmt
        XCTAssertEqual(HerdSun.event(now,calendar:buddhist,location:sample,rise:true),
                       HerdSun.event(now,calendar:utc,location:sample,rise:true))
        XCTAssertEqual(HerdSun.resolve(.auto,now:now,calendar:buddhist,location:sample).explanation,
                       "Auto · local sunrise/sunset")
    }
    func testAuthorizedSolarSunriseInclusiveSunsetExclusive() throws {
        let noon = try date("2026-06-21T12:00:00Z")
        let sample = HerdLocationSample(latitude:51.48,longitude:0,accuracy:20,
                                      timestamp:noon.addingTimeInterval(-43200),authorized:true)
        let rise = try XCTUnwrap(HerdSun.event(noon,calendar:utc,location:sample,rise:true))
        let set = try XCTUnwrap(HerdSun.event(noon,calendar:utc,location:sample,rise:false))
        // Greenwich summer solstice: approximate independent astronomical windows.
        XCTAssertTrue((3...4).contains(utc.component(.hour,from:rise)))
        XCTAssertTrue((20...21).contains(utc.component(.hour,from:set)))
        XCTAssertTrue(HerdSun.resolve(.auto,now:rise.addingTimeInterval(-1),calendar:utc,location:sample).night)
        XCTAssertFalse(HerdSun.resolve(.auto,now:rise,calendar:utc,location:sample).night)
        XCTAssertTrue(HerdSun.resolve(.auto,now:set,calendar:utc,location:sample).night)
        XCTAssertEqual(HerdSun.resolve(.auto,now:noon,calendar:utc,location:sample).explanation,"Auto · local sunrise/sunset")
    }
}
