#!/bin/bash
cd $HOME/.cache/sagascript-bench/bias-298
L=$HOME/.cache/sagascript-bench/longform
wav_fleurs15=$L/fleurs-sv-distinct-15min.wav; wav_fleurs60=$L/fleurs-sv-distinct-60min.wav
wav_riksdag=$HOME/.cache/sagascript-bench/riksdag/debate.wav
for ds in fleurs15 fleurs60 riksdag; do
  eval wav=\$wav_$ds
  ./run.sh $ds-w0 $wav - 0
  for w in 1 2 3 5; do
    ./run.sh $ds-true-w$w $wav $PWD/terms-$ds.txt $w
    ./run.sh $ds-fp-w$w $wav $PWD/terms-unrelated-$ds.txt $w
  done
done
echo done > ALLDONE
