# Test fixtures

Small audio files used by the test suite. Regenerate them with
`./scripts/make-test-fixture.sh`, which is the authoritative recipe.

| File | Contents |
|---|---|
| `tone-tagged.mp3` | 5 s, 44.1 kHz stereo, 64 kbps. ID3v2.3 title/artist/album plus a 256×256 PNG cover in an APIC frame. |
| `tone-untagged.mp3` | The same audio with no ID3 header at all. Exercises the "missing tags are reported as missing, not as an error" contract. |
| `tone-left-only.mp3` | 1 s, 44.1 kHz stereo, 128 kbps, untagged. A 1 kHz tone at amplitude 0.5 in the left channel, silence in the right. |

The audio is a 60 Hz kick decaying every 500 ms under a steady 1 kHz tone, so
loudness visibly rises and falls and the bass band is separable from the mid.
That makes it usable for the end-to-end render test as well as for `probe`.

`tone-left-only.mp3` pins the mono mixdown down to the channel *average*: a
decoder that took channel 0, or summed the channels, would leave the amplitude
at 0.5 instead of halving it.

Do not commit real music here. `scripts/quality.d/30-test-fixtures-are-small-and-cc0.sh`
enforces the size ceiling and the license note below.

## License

Everything in this directory is authored by the avz project: the audio is
synthesized from `sin`/`exp` expressions and the cover art is a generated
gradient. Nothing is sampled, copied, or derived from a third-party work.

These files are dedicated to the public domain under
[CC0 1.0 Universal](https://creativecommons.org/publicdomain/zero/1.0/).
The rest of the repository remains under Apache-2.0.
