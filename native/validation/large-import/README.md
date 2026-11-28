# Cancel and reopen a large scan

Run this desktop test on Linux or Windows with Python 3, Pillow, a graphical session and a scan large enough to display a provisional layer before import finishes. Supply the expected source point count from an independent reader or the source metadata.

```sh
python3 native/validation/large-import/retry.py /path/to/open-pointcloud-studio \
    /path/to/bodleian-tls.e57 190797374
```

The probe starts a separate desktop process with a temporary settings profile, preview cache and index directory. It disables automatic indexing. The default test budget is one million points. Pass `--budget N` to request a different number of points in view. As soon as the first import has a provisional layer, it cancels the import and waits for both the layer and the job to disappear. It reopens the same source in the same process, checks that exactly one layer contains the expected number of remaining points and compares viewport renders before and after rotating the camera. Standard output contains a JSON report. Durations include API polling. The peak memory readings are the desktop process's Linux VmHWM or Windows peak working set, both in KiB.

## Linux field scan

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

## Windows generated grid

The published v2026.48.0 Windows x64 ZIP opened a binary PLY with 500,000,000 XYZ records (6,000,000,123 bytes). At `--budget 50000` the first preview appeared in 1.24 s, cancellation emptied the workspace in 0.67 s and reopening produced one complete layer in 41.78 s. The layer had two million sampled points. Rotating the camera changed 163,142 pixels in the captured viewport. The Windows peak working set through rendering was 655,656 KiB. [Windows run with fixture generator and images](https://github.com/boazeggs/open-pointcloud-studio/actions/runs/45740438873)

The test ran in a fresh application profile on Windows Server 2025 Datacenter, using a virtual SSD, four AMD EPYC logical processors, 16 GiB of RAM and Microsoft Hyper-V Video. The runner had just written the source. At a one-million-point viewport budget the same published package reopened all 500 million source points but returned black viewport images after rotation. Repeated capture eventually failed in the graphics backend. At the application's default budget of 250,000 the rotation capture also failed on another runner of this type. This evidence covers the virtual adapter. Physical Windows 11 hardware remains an open acceptance case. [One-million-point run](https://github.com/boazeggs/open-pointcloud-studio/actions/runs/45740030551) · [Default-budget run](https://github.com/boazeggs/open-pointcloud-studio/actions/runs/45740739733)
