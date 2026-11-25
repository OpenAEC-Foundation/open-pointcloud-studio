# Cancel and reopen a large scan

Run this Linux desktop test with Python 3, Pillow, a graphical session and a scan large enough to display a provisional layer before import finishes. Supply the expected source point count from an independent reader or the source metadata.

```sh
python3 native/validation/large-import/retry.py /path/to/open-pointcloud-studio \
    /path/to/bodleian-tls.e57 190797374
```

The probe starts a separate desktop process with a temporary settings profile, preview cache and index directory. It disables automatic indexing and requests a viewport budget of one million points. As soon as the first import has a provisional layer, it cancels the import and waits for both the layer and the job to disappear. It reopens the same source in the same process, checks that exactly one layer contains the expected number of remaining points and compares viewport renders before and after rotating the camera. Standard output contains a JSON report. Durations include API polling. Peak RSS is the Linux kernel's high-water mark for the desktop process.

## Recorded run

On 25 November 2026 the published v2026.48.0 amd64 AppImage (extracted and run through AppRun) opened the Oxford Spires Bodleian Library TLS scan, `ground_truth_map/bodleian-library/merged-cloud-1cm.e57`. The source was 3,640,843,264 bytes. Its E57 XML metadata gave a single points vector with 190,797,374 records. [Oxford Spires dataset](https://huggingface.co/datasets/ori-drs/oxford_spires_dataset)

| Observation | Result |
| --- | ---: |
| First provisional layer | 250,112 points after 0.23 s |
| Cancellation to empty workspace | 0.13 s |
| Reopen to one complete layer | 20.20 s |
| Remaining source points | 190,797,374 |
| Points in the layer's view sample | 2,000,000 |
| Process peak RSS through rendering | 919,316 KiB |
| Viewport pixels changed by more than 10 RGB levels on at least one channel after rotation | 35,626 |

Single measured run on a shared AMD Ryzen 7 3700U with eight logical CPUs, 13.6 GiB RAM, SSD, integrated Radeon Vega graphics and Mesa RADV 25.2.8. Linux x86-64 kernel 7.0.0-38-generic, headless Wayland compositor, fresh application profile, previously read source in the OS file cache. The test measures the cancel/reopen sequence and sampled rendering. Full index construction and drawing every source point require separate measurements.
