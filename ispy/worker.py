#!/home/muaddib/.cache/pypoetry/virtualenvs/distil-1_vY1VFM-py3.10/bin/python
#
# worker.py
#
# A program that reads from STDIN and executes commands.

from typing import Callable
from pathlib import Path
import time
import sys

MODEL = "distil-whisper/distil-small.en"


def load_model() -> Callable:
    """Load model onto GPU."""

    from transformers import AutoModelForSpeechSeq2Seq, AutoProcessor, pipeline
    import torch

    # TODO: rn this only handles short (<30s) clips efficiently
    # TODO: implement the chunking strategy in distil-whisper README

    device = "cuda:0" if torch.cuda.is_available() else "cpu"
    torch_dtype = torch.float16 if torch.cuda.is_available() else torch.float32

    model = AutoModelForSpeechSeq2Seq.from_pretrained(
        MODEL, torch_dtype=torch_dtype, low_cpu_mem_usage=True, use_safetensors=True
    )
    model.to(device)

    processor = AutoProcessor.from_pretrained(MODEL)

    pipe = pipeline(
        "automatic-speech-recognition",
        model=model,
        tokenizer=processor.tokenizer,
        feature_extractor=processor.feature_extractor,
        max_new_tokens=128,
        torch_dtype=torch_dtype,
        device=device,
    )

    return pipe


def transcribe(pipe, audiofile):
    """Transcribe audio file."""

    import torch

    tic = time.time()
    result = pipe(audiofile)
    torch.cuda.empty_cache()

    return result["text"], time.time() - tic


def print_(*args, **kwargs):
    """Print with flush."""
    print(*args, **kwargs)
    sys.stdout.flush()


def print_transcript(transcript, duration):
    """Print transcript and duration"""
    # print_(f"[transcript] {transcript}\n[duration] {duration:.2f}s")
    print_(f"[transcript] {transcript}")


if __name__ == "__main__":
    pipe = load_model()

    print_("[ready]")

    try:
        for line in sys.stdin:
            if r"\transcribe" in line:
                cmd, audiofile = line.split()

                if Path(audiofile).exists():
                    transcript, duration = transcribe(pipe, audiofile)
                    print_transcript(transcript, duration)
                    sys.stdout.flush()
                else:
                    print_("[error] File does not exist.")

            elif r"\exit" in line:
                del pipe
                break

            else:
                print_("[error] Unknown command.")
    except KeyboardInterrupt:
        pass

    print_("[bye]")
