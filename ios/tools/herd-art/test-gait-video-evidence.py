#!/usr/bin/env python3
"""#480 fixture regression for the gait-video-evidence device metadata.

The generator's manifest must derive its device identity from the SELECTED
simulator (simctl) and its pixel dimensions from the artifacts it actually
captured — never a hardcoded claim. This suite drives the generator's
derivation seam with a fixture simulator that is named/modelled differently
from the historical hardcoded string ("iPhone 16 (393x852 pt; 1179x2556 @3x)
— this host's only simulator"): Corral448Gait / iPhone 16 Pro Max / 1320x2868
px, the real #448 capture identity. A hardcoded generator cannot satisfy the
fixture equality, and every unresolvable identity/dimension case must fail
explicitly ("gait-video-evidence FAIL: ..."). Round 2 (#480 adjudication)
additionally pins that a missing/empty/whitespace/non-string selected
simulator name, a missing udid or device-type field, a blank devicetype
name/modelIdentifier, and a zero pixel dimension are all explicit failures,
while a valid name is stored byte-for-byte. Stdlib only; no simulator, no
capture, no repository mutation.

RED usage against a pre-fix generator copy:
  python3 ios/tools/herd-art/test-gait-video-evidence.py --output <dir> \
      --generator /path/to/pre-fix/gait-video-evidence.py
"""
import argparse
import binascii
import hashlib
import importlib.util
import json
import struct
import sys
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_GENERATOR = HERE / 'gait-video-evidence.py'

FIXTURE_UDID = '44844844-8044-4844-8444-448448448448'
FIXTURE_TYPE = 'com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro-Max'
FIXTURE_RUNTIME = 'com.apple.CoreSimulator.SimRuntime.iOS-26-5'
FIXTURE_DEVICES = {
    'devices': {
        FIXTURE_RUNTIME: [
            {'udid': FIXTURE_UDID, 'name': 'Corral448Gait', 'state': 'Booted',
             'deviceTypeIdentifier': FIXTURE_TYPE},
        ],
    },
}
FIXTURE_DEVICETYPES = {
    'devicetypes': [
        {'identifier': FIXTURE_TYPE, 'name': 'iPhone 16 Pro Max',
         'modelIdentifier': 'iPhone17,2'},
    ],
}
EXPECTED_IDENTITY = {
    'udid': FIXTURE_UDID,
    'name': 'Corral448Gait',
    'model': 'iPhone 16 Pro Max',
    'model_identifier': 'iPhone17,2',
    'device_type_identifier': FIXTURE_TYPE,
}
EXPECTED_DEVICE = {**EXPECTED_IDENTITY, 'pixels': {'width': 1320, 'height': 2868}}
LEGACY_FRAGMENTS = ("this host's only simulator", '393x852', '1179x2556')


def png(width, height, rgb=(7, 7, 7)):
    """Deterministic minimal RGB PNG, same shape as a simctl screenshot."""
    def chunk(kind, payload):
        return (struct.pack('>I', len(payload)) + kind + payload
                + struct.pack('>I', binascii.crc32(kind + payload) & 0xffffffff))

    raw = b''.join(b'\x00' + bytes(rgb) * width for _ in range(height))
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw))
            + chunk(b'IEND', b''))


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_generator(path):
    spec = importlib.util.spec_from_file_location('gait_video_evidence', path)
    if spec is None or spec.loader is None:
        raise SystemExit(f'test harness cannot load generator: {path}')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--generator', type=Path, default=DEFAULT_GENERATOR)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    fixtures = args.output / 'fixtures'
    fixtures.mkdir(exist_ok=True)

    module = load_generator(args.generator)
    source = args.generator.read_text(encoding='utf-8')
    (args.output / 'generator.txt').write_text(
        f'{args.generator}\nsha256={sha256(args.generator)}\n')

    capture_png = fixtures / 'capture-Corral448Gait-1320x2868.png'
    capture_png.write_bytes(png(1320, 2868))
    second_png = fixtures / 'capture-second-1320x2868.png'
    second_png.write_bytes(png(1320, 2868, rgb=(9, 9, 9)))
    other_png = fixtures / 'capture-iPhone16-1179x2556.png'
    other_png.write_bytes(png(1179, 2556))
    corrupt_png = fixtures / 'capture-corrupt.png'
    corrupt_png.write_bytes(b'\x89PNG\r\n\x1a\n' + b'no IHDR here, not a screenshot')
    missing_png = fixtures / 'capture-missing.png'  # deliberately never written
    zero_width_png = fixtures / 'capture-zero-width-0x2868.png'
    zero_width_png.write_bytes(png(0, 2868))
    zero_height_png = fixtures / 'capture-zero-height-1320x0.png'
    zero_height_png.write_bytes(png(1320, 0))

    results = []

    def record(name, exercise):
        try:
            detail = exercise()
            status = 'PASS'
        except Exception as error:  # noqa: BLE001 - every failure kind is reported
            status = 'FAIL'
            detail = f'{type(error).__name__}: {error}'
        (args.output / f'{name}.log').write_text(f'{status} {name}: {detail}\n')
        results.append({'name': name, 'status': status, 'detail': str(detail)})
        (args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
        print(f'{name} {status} {detail}', flush=True)
        return status == 'PASS'

    def seam(name):
        function = getattr(module, name, None)
        if function is None:
            raise AssertionError(
                f'generator exposes no {name}: device metadata is not derived '
                f'(fixture {FIXTURE_UDID})')
        return function

    def identity_from_fixture():
        device = seam('find_device')(FIXTURE_DEVICES, FIXTURE_UDID)
        return seam('device_identity')(device, FIXTURE_DEVICETYPES)

    def expect_fail(needle, exercise):
        try:
            exercise()
        except SystemExit as error:
            message = str(error)
            assert needle in message, f'expected {needle!r} in {message!r}'
            return message
        raise AssertionError(f'expected explicit failure containing {needle!r}')

    def check_fixture_discriminates():
        assert EXPECTED_IDENTITY['name'] == 'Corral448Gait' != 'iPhone 16'
        assert EXPECTED_DEVICE['pixels'] == {'width': 1320, 'height': 2868}
        for fragment in LEGACY_FRAGMENTS:
            assert fragment not in json.dumps(EXPECTED_DEVICE)
        return 'fixture Corral448Gait/iPhone 16 Pro Max/1320x2868 differs from the legacy claim'

    def check_source_wiring():
        assert source.count('def device_metadata(') == 1, 'device_metadata definition'
        assert source.count('device_metadata(') == 2, \
            'device_metadata must be defined once and called once (assembly)'
        assert 'manifest["device"] = device_metadata(' in source, 'manifest device assembly'
        for fragment in LEGACY_FRAGMENTS:
            assert fragment not in source, f'hardcoded metadata fragment: {fragment!r}'
        return 'manifest device assembled from derived identity + captured pixels'

    def check_identity_derivation():
        identity = identity_from_fixture()
        assert identity == EXPECTED_IDENTITY, f'identity {identity}'
        return json.dumps(identity, sort_keys=True)

    def check_device_metadata_golden():
        pixels = seam('capture_pixels')([capture_png, second_png])
        assert pixels == (1320, 2868), f'pixels {pixels}'
        device = seam('device_metadata')(identity_from_fixture(), pixels)
        assert device == EXPECTED_DEVICE, f'device {device}'
        assert device != {'name': 'iPhone 16', 'pixels': {'width': 1179, 'height': 2556}}
        return json.dumps(device, sort_keys=True)

    def check_pixels_disagree_fails():
        return expect_fail('disagree', lambda: seam('capture_pixels')([capture_png, other_png]))

    def check_pixels_empty_fails():
        return expect_fail('no captured screenshot artifacts', lambda: seam('capture_pixels')([]))

    def check_pixels_corrupt_fails():
        return expect_fail('not a PNG', lambda: seam('capture_pixels')([corrupt_png]))

    def check_pixels_missing_file_fails():
        return expect_fail('cannot read', lambda: seam('capture_pixels')([missing_png]))

    def check_unknown_udid_fails():
        return expect_fail('absent from the simctl device list',
                           lambda: seam('find_device')(FIXTURE_DEVICES, 'no-such-udid'))

    def check_not_booted_fails():
        device = dict(FIXTURE_DEVICES['devices'][FIXTURE_RUNTIME][0], state='Shutdown')
        return expect_fail('booted by the caller',
                           lambda: seam('device_identity')(device, FIXTURE_DEVICETYPES))

    def check_unknown_device_type_fails():
        device = dict(FIXTURE_DEVICES['devices'][FIXTURE_RUNTIME][0],
                      deviceTypeIdentifier='com.apple.CoreSimulator.SimDeviceType.iPhone-99')
        return expect_fail('cannot resolve device type',
                           lambda: seam('device_identity')(device, FIXTURE_DEVICETYPES))

    # Round 2 (#480 adjudication): a missing/blank selected-simulator name must
    # be an explicit failure, not a silently empty manifest field.
    def mutated_device(**changes):
        return dict(FIXTURE_DEVICES['devices'][FIXTURE_RUNTIME][0], **changes)

    def check_name_missing_fails():
        device = mutated_device()
        device.pop('name')
        return expect_fail('no usable name',
                           lambda: seam('device_identity')(device, FIXTURE_DEVICETYPES))

    def check_name_empty_fails():
        return expect_fail('no usable name',
                           lambda: seam('device_identity')(mutated_device(name=''), FIXTURE_DEVICETYPES))

    def check_name_whitespace_fails():
        return expect_fail('no usable name',
                           lambda: seam('device_identity')(mutated_device(name='   '), FIXTURE_DEVICETYPES))

    def check_name_non_string_fails():
        return expect_fail('no usable name',
                           lambda: seam('device_identity')(mutated_device(name=16), FIXTURE_DEVICETYPES))

    def check_name_preserved_verbatim():
        identity = seam('device_identity')(mutated_device(name=' Corral448Gait '), FIXTURE_DEVICETYPES)
        assert identity['name'] == ' Corral448Gait ', f'name {identity["name"]!r}'
        return 'valid name stored byte-for-byte (no strip/normalization)'

    def check_udid_missing_fails():
        device = mutated_device()
        device.pop('udid')
        return expect_fail('no usable udid',
                           lambda: seam('device_identity')(device, FIXTURE_DEVICETYPES))

    def check_device_type_missing_fails():
        device = mutated_device()
        device.pop('deviceTypeIdentifier')
        return expect_fail('no usable device type identifier',
                           lambda: seam('device_identity')(device, FIXTURE_DEVICETYPES))

    def check_devicetype_name_blank_fails():
        types = {'devicetypes': [dict(FIXTURE_DEVICETYPES['devicetypes'][0], name=' ')]}
        return expect_fail('no usable name',
                           lambda: seam('device_identity')(mutated_device(), types))

    def check_devicetype_model_identifier_blank_fails():
        types = {'devicetypes': [dict(FIXTURE_DEVICETYPES['devicetypes'][0], modelIdentifier='')]}
        return expect_fail('no usable modelIdentifier',
                           lambda: seam('device_identity')(mutated_device(), types))

    def check_pixels_zero_width_fails():
        return expect_fail('zero pixel dimension',
                           lambda: seam('capture_pixels')([zero_width_png]))

    def check_pixels_zero_height_fails():
        return expect_fail('zero pixel dimension',
                           lambda: seam('capture_pixels')([zero_height_png]))

    checks = [
        ('fixture-discriminates', check_fixture_discriminates),
        ('source-wiring', check_source_wiring),
        ('identity-derivation', check_identity_derivation),
        ('device-metadata-golden', check_device_metadata_golden),
        ('pixels-disagree-fails', check_pixels_disagree_fails),
        ('pixels-empty-fails', check_pixels_empty_fails),
        ('pixels-corrupt-fails', check_pixels_corrupt_fails),
        ('pixels-missing-file-fails', check_pixels_missing_file_fails),
        ('unknown-udid-fails', check_unknown_udid_fails),
        ('not-booted-fails', check_not_booted_fails),
        ('unknown-device-type-fails', check_unknown_device_type_fails),
        ('name-missing-fails', check_name_missing_fails),
        ('name-empty-fails', check_name_empty_fails),
        ('name-whitespace-fails', check_name_whitespace_fails),
        ('name-non-string-fails', check_name_non_string_fails),
        ('name-preserved-verbatim', check_name_preserved_verbatim),
        ('udid-missing-fails', check_udid_missing_fails),
        ('device-type-missing-fails', check_device_type_missing_fails),
        ('devicetype-name-blank-fails', check_devicetype_name_blank_fails),
        ('devicetype-model-identifier-blank-fails', check_devicetype_model_identifier_blank_fails),
        ('pixels-zero-width-fails', check_pixels_zero_width_fails),
        ('pixels-zero-height-fails', check_pixels_zero_height_fails),
    ]
    ok = True
    for name, exercise in checks:
        ok = record(name, exercise) and ok
    if not ok:
        print(f'FAIL: gait metadata fixture checks against {args.generator}', flush=True)
        raise SystemExit(1)
    print(f'PASS: {len(results)} gait metadata fixture checks against {args.generator}', flush=True)


if __name__ == '__main__':
    main()
