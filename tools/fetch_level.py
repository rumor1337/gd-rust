"""Download a GD level (and its song, and optionally a macro) into the repo.

Writes levels/<id>.gmd (decoded level string), levels/<id>.json
({"name", "song"}: song = repo-relative mp3 path or null) and the song mp3
under audio/. Existing files are kept, so re-running only fills gaps.

Level + song come from GD's own server API. There is no public macro index
(the macros in macros/ were found by hand), so a macro needs a URL:
it is saved as macros/<id>.gdr2.

usage: python3 -I tools/fetch_level.py <level id> [macro url] [gd install dir]
"""
import base64
import json
import os
import shutil
import sys
import urllib.parse
import urllib.request
import zlib

API = 'https://www.boomlings.com/database/'
SECRET = 'Wmfd2893gb7'  # GD's public client secret, sent by every game client
# official song index (level key 12) -> file in GD's Resources
OFFICIAL = ['StereoMadness', 'BackOnTrack', 'Polargeist', 'DryOut', 'BaseAfterBase', 'CantLetGo', 'Jumper',
            'TimeMachine', 'Cycles', 'xStep', 'Clutterfunk', 'TheoryOfEverything', 'Electroman', 'Clubstep',
            'Electrodynamix', 'HexagonForce', 'BlastProcessing', 'TheoryOfEverything2', 'GeometricalDominator',
            'Deadlocked', 'Fingerdash', 'Dash']


def post(endpoint, **form):
    form['secret'] = SECRET
    # GD's server rejects requests that carry a User-Agent
    req = urllib.request.Request(API + endpoint, urllib.parse.urlencode(form).encode(), headers={'User-Agent': ''})
    body = urllib.request.urlopen(req, timeout=30).read().decode()
    if body.startswith('-'):
        sys.exit(f'{endpoint}: server returned {body} (no such level/song?)')
    return body


def download(url, path, magic=None):
    if url.startswith('http://'):
        url = 'https://' + url[len('http://'):]  # old NG song URLs; the CDNs serve https too
    if not url.startswith('https://'):
        sys.exit(f'refusing non-https download: {url}')
    data = urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': 'gd-rust'}), timeout=120).read()
    if magic and not any(data.startswith(m) for m in magic):
        sys.exit(f'{url}: not the expected file type')
    tmp = path + '.part'
    open(tmp, 'wb').write(data)
    os.replace(tmp, path)


def parse_level_response(body):
    """'k:v:k:v#hashes' -> (fields, decoded level string)."""
    parts = body.split('#')[0].split(':')
    d = dict(zip(parts[::2], parts[1::2]))
    raw = d['4']
    level = zlib.decompress(base64.urlsafe_b64decode(raw + '=' * (-len(raw) % 4)), 47).decode()
    return d, level


def song_url(song_id):
    parts = post('getGJSongInfo.php', songID=song_id).split('~|~')
    return urllib.parse.unquote(dict(zip(parts[::2], parts[1::2]))['10'])


def main(level_id, macro_url=None, gd_dir=os.path.expanduser('~/Documents/gd-clone')):
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    gmd, meta_path = f'{root}/levels/{level_id}.gmd', f'{root}/levels/{level_id}.json'
    fields, level = parse_level_response(post('downloadGJLevel22.php', levelID=level_id))
    if not os.path.exists(gmd):
        open(gmd, 'w').write(level)
    custom = int(fields.get('35') or 0)
    if custom >= 10_000_000:
        # shortcut: 2.2 music-library songs are not fetched (none of our levels use one)
        print(f'song {custom} is from the 2.2 music library: not fetched, level plays silent')
        song = None
    elif custom:
        song = f'audio/{custom}.mp3'
        if not os.path.exists(f'{root}/{song}'):
            download(song_url(custom), f'{root}/{song}', magic=[b'ID3', b'\xff\xfb', b'\xff\xf3', b'\xff\xf2'])
    else:
        name = OFFICIAL[int(fields.get('12') or 0)] if int(fields.get('12') or 0) < len(OFFICIAL) else None
        src = f'{gd_dir}/Resources/{name}.mp3'
        song = f'audio/{name}.mp3' if name and os.path.exists(src) else None
        if song and not os.path.exists(f'{root}/{song}'):
            shutil.copy(src, f'{root}/{song}')
    json.dump({'name': fields.get('2', str(level_id)), 'song': song}, open(meta_path, 'w'))
    if macro_url:
        download(macro_url, f'{root}/macros/{level_id}.gdr2')
    print(f'{fields.get("2")}: level {gmd}, song {song}, macro {"saved" if macro_url else "none given"}')


def _selftest():
    lvl = 'kS38,1_0|;1,1,2,15,3,15;'
    raw = base64.urlsafe_b64encode(zlib.compress(lvl.encode())).decode().rstrip('=')
    d, s = parse_level_response(f'1:42:2:Test:4:{raw}#hash#more')
    assert (d['1'], d['2'], s) == ('42', 'Test', lvl), (d, s)


if __name__ == '__main__':
    _selftest()
    if len(sys.argv) < 2 or not sys.argv[1].isdigit():
        sys.exit(__doc__)
    main(sys.argv[1], *sys.argv[2:4])
