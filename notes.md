ok. mail kinda sucks

my inclinations about it are vimmy; we want mutt but, like, fresh and clean
the noctmalia of mutt. the ghostty/superlogical of mutt. 

- we render md
  - this means that our users sometimes write each other in md
  - we can recycle some basalt here
- we like screening
  - it's just a good idea. we should encourage users to define rules here,
    and help them push those rules to the mailhost where available. we can
    also express them in terms of thunderbird in many places.
- we like privacy
  - we block trackers by default, we load nothing from the internet by default
    we render html in a cautious way, we highlight funny looking headers
  - we do as much of our sec shit in-client as we can, we don't inherently
    trust the mailhost there
- we make mail a directory full of files like it used to be in unix
  - this simplifies trimming, etc.
  - i'm not actually sure what we get from the tbird backend here, syncing and all that shit probably. i'm not actually rock solid on this
- we respect our user's intelligence and also hope to augment it
  - we promote features once or twice, when the user naturally floats near them
  - we want to streamline email with the user using normal email affordances
  - part of impl in terms of tbird is that tbird is an excellent representation of normal email affordances
- we look slick
  - we have a contacts app that expresses our general design language already, and we follow it
  - if an ordinary email or tbird feature is 'ugly' we question it
- we defer to tbird where possible
  - ideally we don't need a db. if we'd need some sort of state outside tbird for a feature, that's reason to question the feature
- we have juice
  - our existing design language includes bounce, vibes, we preserve and exetend that
- we are modular
  - when we add juice, we add it to in lib with design components, so contacts, mail, and calendar may all benefit.
  - our juice composes (ex you can have a component, a bouncy<component>, a styled<component>, a bouncy<styled<component>>, so each juice is optional
  - we're ultimately hoping to end up with a set of slick iced stuff that works well in noctalia systems, in addition to solving mail
- we conserve mental energy
  - mail sucks because it's taxing
  - we encourage user to filter, autofolder, et cetera, until reading mail is kinda nice, because it's only useful things

