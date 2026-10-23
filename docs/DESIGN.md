# Muninn appearance

Classified Terminal is an optional theme with an editorial, technical-document
appearance. System appearance remains available.

The theme carries the source hierarchy: large Cinzel titles, small numbered mono
section labels, Inter body copy, IBM Plex Mono readings, sparse hairline separators,
an anchored sidebar identity, underline navigation and subtle dossier corner marks.
Controls use restrained 3px corners. Channel policies are readable summaries with
editing disclosed on request, rather than an always-visible wall of form fields.

Dark uses charcoal surfaces; light uses the site's warm paper palette. Red, amber,
blue and teal are independent accent choices. Amber matches the original Movement
identity. Text/status labels accompany color, focus remains visible, and reduced
motion is respected. Fonts and their OFL licenses are bundled; no remote font calls.

The requested
[UI UX Pro Max skill](https://github.com/nextlevelbuilder/ui-ux-pro-max-skill) was read
at commit 56ea9cf and its accessibility, navigation, forms and progressive disclosure
guidance applied. Its generated landing-page/LMS layouts did not match this desktop
media application and were not adopted. The user's existing visual language governs
the design. No private stylesheet is needed to build it.

Logo review: [six selectable directions](../design/logo-options.html). This is a
review page; none of its candidate marks replaces the app identity until selected.
Round two: [01A–01F folded-wing variations](../design/logo-options-wing.html), with
the original 01 kept as a reference.
Round three: [03A–03F angular perched ravens](../design/logo-options-raven.html),
using the user's raven photograph for anatomy and posture rather than repeating
the folded-wing symbol. Includes full-body, faceted, bust, long-tail, outline and
heavy-profile compositions. The photograph itself is not copied into the package.

Latest direction: [photo-based raven preview](../design/logo-photo.html), with a
[transparent PNG](../design/raven-photo-styled.png). This styles the supplied
photograph directly, preserving its heavy beak, broad body and long tail in red
and burgundy angular planes. As of 0.2.5 this is the app identity: the sidebar and
favicon use a transparent export; the Dock icon uses the same raven on a dark tile.
`build/export_icons.py` reproduces all committed PNG, ICO and ICNS exports using
ImageMagick. Application builds use the committed assets directly.
