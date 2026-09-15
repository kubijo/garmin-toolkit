#!/usr/bin/env python3
"""Normalize the pinned trackeR sample activities into publishable demo inputs."""

from __future__ import annotations

import csv
import gzip
import hashlib
import io
import json
import math
import sys
import tarfile
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from datetime import datetime
from itertools import pairwise
from pathlib import Path

ARCHIVE_SHA256 = '61ae7adb5fc53b8342af9d263ba59d0d8006c5d3a41995e107dc6ac5d4dc24b6'


@dataclass(frozen=True)
class Source:
    output: str
    member: str
    sha256: str
    kind: str


SOURCES = (
    Source(
        'forest-run.csv.gz',
        'trackeR/inst/extdata/tcx/2013-06-01-183220.TCX.gz',
        'bc32dc93878e71c5eece8f0f2c54897946429369431817b4fe17bcc206f7212a',
        'tcx',
    ),
    Source(
        'coastal-run.csv.gz',
        'trackeR/inst/extdata/tcx/2013-06-08-090442.TCX.gz',
        '0ea8aa3931e4c14626c0e613875f3829dd3f48a412762e5578a10b49210d90f6',
        'tcx',
    ),
    Source(
        'city-run.csv.gz',
        'trackeR/inst/extdata/gpx/20170708-154835-Run.gpx.gz',
        'c3f6018ceb9d6db6b61a7133d75b84307252fa8ff3a8b5830ae1c49f30720c92',
        'gpx',
    ),
    Source(
        'city-ride.csv.gz',
        'trackeR/inst/extdata/gpx/20170709-151453-Ride.gpx.gz',
        'ce9bb79746b71546187d0b2d1a348779dad62d5fe2a614fcc6e3e462d435914f',
        'gpx',
    ),
    Source(
        'open-water-swim.csv.gz',
        'trackeR/inst/extdata/gpx/20170714-143644-Swim.gpx.gz',
        '032cbc586782f0143b8a4ab19f97a5f51837bb884ca6e342ade31678e35fa7a2',
        'gpx',
    ),
    Source(
        'indoor-power-ride.csv.gz',
        'trackeR/inst/extdata/json/2017_04_24_10_18_45.json.gz',
        '9ea6c6f76920df585a6a6bdfb39e77baf89bf00d0bd8bc22652b54a5bb5122cf',
        'json',
    ),
)


@dataclass
class Sample:
    elapsed_seconds: int
    latitude: float | None = None
    longitude: float | None = None
    elevation: float | None = None
    distance: float | None = None
    speed: float | None = None
    heart_rate: int | None = None
    cadence: int | None = None
    power: int | None = None
    temperature: float | None = None
    lap_end: bool = False


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def local_name(element: ET.Element) -> str:
    return element.tag.rsplit('}', 1)[-1]


def descendant_text(element: ET.Element, name: str) -> str | None:
    for descendant in element.iter():
        if local_name(descendant) == name and descendant.text is not None:
            return descendant.text.strip()
    return None


def optional_float(element: ET.Element, name: str) -> float | None:
    value = descendant_text(element, name)
    return None if value is None else float(value)


def optional_int(element: ET.Element, name: str) -> int | None:
    value = descendant_text(element, name)
    return None if value is None else int(float(value))


def timestamp(value: str) -> datetime:
    return datetime.fromisoformat(value.replace('Z', '+00:00'))


def parse_tcx(data: bytes) -> list[Sample]:
    root = ET.parse(gzip.GzipFile(fileobj=io.BytesIO(data))).getroot()
    samples: list[Sample] = []
    first_time: datetime | None = None
    distance_offset = 0.0
    previous_raw_distance: float | None = None

    for lap in (element for element in root.iter() if local_name(element) == 'Lap'):
        lap_samples: list[Sample] = []
        for point in (element for element in lap.iter() if local_name(element) == 'Trackpoint'):
            point_time = timestamp(required_text(point, 'Time'))
            if first_time is None:
                first_time = point_time
            raw_distance = optional_float(point, 'DistanceMeters')
            if (
                raw_distance is not None
                and previous_raw_distance is not None
                and raw_distance + 1.0 < previous_raw_distance
            ):
                distance_offset += previous_raw_distance
            if raw_distance is not None:
                previous_raw_distance = raw_distance
            latitude = optional_float(point, 'LatitudeDegrees')
            longitude = optional_float(point, 'LongitudeDegrees')
            lap_samples.append(
                Sample(
                    elapsed_seconds=round((point_time - first_time).total_seconds()),
                    latitude=latitude,
                    longitude=longitude,
                    elevation=optional_float(point, 'AltitudeMeters'),
                    distance=None if raw_distance is None else distance_offset + raw_distance,
                    speed=optional_float(point, 'Speed'),
                    heart_rate=optional_int(point, 'Value'),
                    cadence=optional_int(point, 'RunCadence') or optional_int(point, 'Cadence'),
                )
            )
        if lap_samples:
            lap_samples[-1].lap_end = True
            samples.extend(lap_samples)
    return samples


def required_text(element: ET.Element, name: str) -> str:
    value = descendant_text(element, name)
    if value is None:
        raise ValueError(f'missing {name}')
    return value


def parse_gpx(data: bytes) -> list[Sample]:
    root = ET.parse(gzip.GzipFile(fileobj=io.BytesIO(data))).getroot()
    samples: list[Sample] = []
    first_time: datetime | None = None
    for point in (element for element in root.iter() if local_name(element) == 'trkpt'):
        point_time = timestamp(required_text(point, 'time'))
        if first_time is None:
            first_time = point_time
        samples.append(
            Sample(
                elapsed_seconds=round((point_time - first_time).total_seconds()),
                latitude=float(point.attrib['lat']),
                longitude=float(point.attrib['lon']),
                elevation=optional_float(point, 'ele'),
                heart_rate=optional_int(point, 'hr'),
                cadence=optional_int(point, 'cad'),
                power=optional_int(point, 'power'),
                temperature=optional_float(point, 'atemp'),
            )
        )
    add_derived_distance_and_speed(samples)
    add_even_laps(samples, 4)
    return samples


def parse_json(data: bytes) -> list[Sample]:
    with gzip.open(io.BytesIO(data), mode='rt', encoding='utf-8-sig') as source:
        document = json.load(source)['RIDE']
    lap_ends = {interval['STOP'] - 1 for interval in document['INTERVALS']}
    samples = [
        Sample(
            elapsed_seconds=point['SECS'],
            distance=point['KM'] * 1_000.0,
            speed=point['KPH'] / 3.6,
            cadence=point.get('CAD'),
            power=point.get('WATTS'),
            lap_end=point['SECS'] in lap_ends,
        )
        for point in document['SAMPLES']
    ]
    samples[-1].lap_end = True
    return samples


def add_derived_distance_and_speed(samples: list[Sample]) -> None:
    distance = 0.0
    speeds = [0.0]
    samples[0].distance = 0.0
    for previous, current in pairwise(samples):
        segment = haversine(previous, current)
        distance += segment
        current.distance = distance
        elapsed = current.elapsed_seconds - previous.elapsed_seconds
        speeds.append(0.0 if elapsed <= 0 else segment / elapsed)

    for index, sample in enumerate(samples):
        start = max(0, index - 3)
        end = min(len(speeds), index + 4)
        sample.speed = sum(speeds[start:end]) / (end - start)


def haversine(start: Sample, end: Sample) -> float:
    if None in (start.latitude, start.longitude, end.latitude, end.longitude):
        return 0.0
    latitude_1 = math.radians(start.latitude)
    latitude_2 = math.radians(end.latitude)
    latitude_delta = latitude_2 - latitude_1
    longitude_delta = math.radians(end.longitude - start.longitude)
    a = (
        math.sin(latitude_delta / 2.0) ** 2
        + math.cos(latitude_1) * math.cos(latitude_2) * math.sin(longitude_delta / 2.0) ** 2
    )
    return 6_371_008.8 * 2.0 * math.atan2(math.sqrt(a), math.sqrt(1.0 - a))


def add_even_laps(samples: list[Sample], count: int) -> None:
    for ordinal in range(1, count + 1):
        index = min(len(samples) - 1, ordinal * len(samples) // count - 1)
        samples[index].lap_end = True
    samples[-1].lap_end = True


def downsample(samples: list[Sample]) -> list[Sample]:
    selected: list[Sample] = []
    last_elapsed = -5
    for index, sample in enumerate(samples):
        if index == 0 or sample.lap_end or sample.elapsed_seconds - last_elapsed >= 5:
            selected.append(sample)
            last_elapsed = sample.elapsed_seconds
    if selected[-1] is not samples[-1]:
        selected.append(samples[-1])
    selected[-1].lap_end = True
    return selected


def render(value: float | None, precision: int | None = None) -> str:
    if value is None:
        return ''
    if precision is None:
        return str(value)
    return f'{value:.{precision}f}'.rstrip('0').rstrip('.')


def write_output(path: Path, samples: list[Sample]) -> None:
    buffer = io.StringIO(newline='')
    writer = csv.writer(buffer, lineterminator='\n')
    writer.writerow(
        (
            'elapsed_seconds',
            'latitude_degrees',
            'longitude_degrees',
            'elevation_meters',
            'distance_meters',
            'speed_meters_per_second',
            'heart_rate_bpm',
            'cadence_rpm',
            'power_watts',
            'temperature_celsius',
            'lap_end',
        )
    )
    for sample in samples:
        writer.writerow(
            (
                sample.elapsed_seconds,
                render(sample.latitude, 7),
                render(sample.longitude, 7),
                render(sample.elevation, 1),
                render(sample.distance, 2),
                render(sample.speed, 3),
                render(sample.heart_rate),
                render(sample.cadence),
                render(sample.power),
                render(sample.temperature, 1),
                int(sample.lap_end),
            )
        )
    with (
        path.open('wb') as raw,
        gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed,
    ):
        compressed.write(buffer.getvalue().encode())


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f'usage: {sys.argv[0]} trackeR_1.6.1.tar.gz')
    archive_path = Path(sys.argv[1])
    archive_bytes = archive_path.read_bytes()
    if digest(archive_bytes) != ARCHIVE_SHA256:
        raise SystemExit('trackeR archive SHA-256 does not match the approved source')

    output_directory = Path(__file__).resolve().parent / 'recordings'
    output_directory.mkdir(exist_ok=True)
    parsers = {'tcx': parse_tcx, 'gpx': parse_gpx, 'json': parse_json}
    with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode='r:gz') as archive:
        for source in SOURCES:
            member = archive.extractfile(source.member)
            if member is None:
                raise SystemExit(f'missing archive member {source.member}')
            source_bytes = member.read()
            if digest(source_bytes) != source.sha256:
                raise SystemExit(f'SHA-256 mismatch for {source.member}')
            samples = downsample(parsers[source.kind](source_bytes))
            write_output(output_directory / source.output, samples)
            print(
                f'{source.output}: {len(samples)} samples, '
                f'{samples[-1].elapsed_seconds} s, '
                f'{samples[-1].distance or 0.0:.2f} m'
            )


if __name__ == '__main__':
    main()
