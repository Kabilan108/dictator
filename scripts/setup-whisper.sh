#! /bin/bash

set -e          # exit script if any errors are thrown
set -u          # unset variables throw errors
set -o pipefail # pipe return value = status of last command with exit status != 0
# set -x          # print commands as they are executed

ROOT=$(dirname $(dirname $(realpath $0)))
BIN="$ROOT/bin"

BUILD_FLAGS=""
if [[ "$(uname)" == "Linux" ]]; then
  if command -v nvcc &> /dev/null; then
    BUILD_FLAGS="-DGGML_CUDA=1"
  elif ldconfig -p | grep -q libopenblas; then
    BUILD_FLAGS="-DGGML_BLAS=1"
  fi
fi

if [ -d $BIN ]; then
  rm -r $BIN
fi

mkdir -p $BIN && cd $BIN
wget https://github.com/ggerganov/whisper.cpp/archive/refs/tags/v1.7.4.zip
unzip v1.7.4.zip && cd whisper.cpp-1.7.4

cmake -B build $BUILD_FLAGS
cmake --build build -j $(($(nproc) - 2)) --config Release

cd .. && rm v1.7.4.zip
ln -s $BIN/whisper.cpp-1.7.4/build/bin/whisper-server $BIN/whisper-server
