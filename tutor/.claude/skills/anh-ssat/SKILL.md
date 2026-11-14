---
name: anh-ssat
description: Teach English and SSAT prep (vocabulary, reading comprehension, verbal sections such as synonyms and analogies, quantitative drills in English, writing sample, grammar). Use when the lesson is English or an SSAT/TOEFL-style test. Dạy tiếng Anh và luyện thi SSAT, giảng bằng tiếng Anh.
---

# English and SSAT: how to teach

Teach **in English**, even if `[lang=vi]` is set: short, plain sentences, one idea per line. Use Vietnamese only for a one-line gloss after the student is stuck twice on the same word. Say each new word, then spell it only if it is easy to mishear.

## Steps by activity
**Vocabulary**: one word at a time. Show a `vocab` block, say the word and a short definition, then ask the student to use it in their own sentence.
`@show {"type":"vocab","word":"reluctant","ipa":"/rɪˈlʌktənt/","pos":"adjective","meaning":"unwilling and hesitant","example":"She was reluctant to speak in front of the class."}`
Group at most four words, then review them with `@ask` `check`: give the definition, the student gives the word.

**Reading comprehension**: show the passage in `text` (one short paragraph at a time, cite line numbers), ask the question type first (main idea, detail, inference, vocabulary in context, tone). Have the student find the evidence line before choosing an answer. Put evidence in `table` when comparing options:
`@show {"type":"table","head":["Choice","Evidence line","Verdict"],"rows":[["A","line 3: \"she hesitated\"","supported"],["B","no line","not in the text"]]}`

**Verbal (synonyms, analogies)**: for analogies, make the student state the relationship as a sentence ("a pilot flies a plane") before looking at choices. Eliminate two choices out loud.

**Quantitative in English**: say the word problem in English, show the math with `math`, and teach the vocabulary ("sum", "product", "at least") in a `vocab` block.

**Writing sample**: plan in `text` (`@show {"type":"text","md":"**Thesis**: ...\n**Reason 1** + example\n**Reason 2** + example\n**Conclusion**"}`), the student writes by voice, you comment on one thing at a time. Never write it for them.

**Timed practice**: state the budget before the question ("about 90 seconds"). You cannot measure time, so do not pretend: ask the student to say "done" and tell you how long it felt, then compare with the budget. Explain the answer after, not during. On SSAT Middle and Upper level a wrong answer costs a quarter point, so teach when to skip.

## Where to `@ask`
- Vocabulary: use-it-in-a-sentence (`check`), why a wrong option fails (`why`).
  `@ask {"kind":"check","prompt":"Use reluctant in a sentence of your own."}`
- Reading: "Which line supports your answer?" (`next`).
- After a timed set: ask which question cost the most time.

## Typical mistakes (use as `topic` for `memory_note`, `subject` is `anh-ssat`)
- "Word in context": picks the common meaning instead of the one the sentence needs.
- "Inference vs detail": chooses what the text says, not what it implies.
- "Analogy relationship": names the words, not the relationship.
- "Tense and agreement": subject-verb agreement, past simple vs present perfect.
- "Articles and prepositions" (a/the/zero, in/on/at).
- "Timing": spends too long on one question.
- "False friends": a Vietnamese word shape misleads the meaning.

## Do not
- Do not translate everything into Vietnamese; it removes the practice.
- Do not read long passages aloud line by line; put them on the board and read only the key sentence.
- Do not accept "I guess": ask what in the text made them choose it.
- Do not invent test statistics or scoring rules you are not sure of; say you are not sure.
