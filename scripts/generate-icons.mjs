import { Resvg } from '@resvg/resvg-js';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(__dirname, '..');
const svgPath = path.join(rootDir, 'public', 'logo.svg');
const iconsDir = path.join(rootDir, 'src-tauri', 'icons');

const svgData = fs.readFileSync(svgPath, 'utf-8');

// All sizes needed for Tauri
const sizes = [
  { name: '32x32.png', size: 32 },
  { name: '64x64.png', size: 64 },
  { name: '128x128.png', size: 128 },
  { name: '128x128@2x.png', size: 256 },
  { name: 'icon.png', size: 512 },
  // Windows Store logos
  { name: 'Square30x30Logo.png', size: 30 },
  { name: 'Square44x44Logo.png', size: 44 },
  { name: 'Square71x71Logo.png', size: 71 },
  { name: 'Square89x89Logo.png', size: 89 },
  { name: 'Square107x107Logo.png', size: 107 },
  { name: 'Square142x142Logo.png', size: 142 },
  { name: 'Square150x150Logo.png', size: 150 },
  { name: 'Square284x284Logo.png', size: 284 },
  { name: 'Square310x310Logo.png', size: 310 },
  { name: 'StoreLogo.png', size: 50 },
];

for (const { name, size } of sizes) {
  const resvg = new Resvg(svgData, {
    fitTo: { mode: 'width', value: size },
  });
  const pngData = resvg.render();
  const pngBuffer = pngData.asPng();
  const outPath = path.join(iconsDir, name);
  fs.writeFileSync(outPath, pngBuffer);
  console.log(`Generated ${name} (${size}x${size})`);
}

// Generate ICO file (contains 16, 32, 48, 256 px)
// ICO format: header + directory entries + PNG data
const icoSizes = [16, 32, 48, 256];
const pngBuffers = icoSizes.map(size => {
  const resvg = new Resvg(svgData, { fitTo: { mode: 'width', value: size } });
  return resvg.render().asPng();
});

function createIco(pngDataArr, icoSizesArr) {
  const count = pngDataArr.length;
  const headerSize = 6;
  const dirEntrySize = 16;
  const dirSize = dirEntrySize * count;
  let dataOffset = headerSize + dirSize;

  // Header: reserved(2) + type(2) + count(2)
  const header = Buffer.alloc(headerSize);
  header.writeUInt16LE(0, 0);      // reserved
  header.writeUInt16LE(1, 2);      // type = ICO
  header.writeUInt16LE(count, 4);  // image count

  const dirEntries = [];
  const offsets = [];
  for (let i = 0; i < count; i++) {
    const sz = icoSizesArr[i];
    const entry = Buffer.alloc(dirEntrySize);
    entry.writeUInt8(sz >= 256 ? 0 : sz, 0);    // width (0 = 256)
    entry.writeUInt8(sz >= 256 ? 0 : sz, 1);    // height (0 = 256)
    entry.writeUInt8(0, 2);                       // color palette
    entry.writeUInt8(0, 3);                       // reserved
    entry.writeUInt16LE(1, 4);                    // color planes
    entry.writeUInt16LE(32, 6);                   // bits per pixel
    entry.writeUInt32LE(pngDataArr[i].length, 8); // data size
    entry.writeUInt32LE(dataOffset, 12);           // data offset
    dirEntries.push(entry);
    offsets.push(dataOffset);
    dataOffset += pngDataArr[i].length;
  }

  return Buffer.concat([header, ...dirEntries, ...pngDataArr]);
}

const icoBuffer = createIco(pngBuffers, icoSizes);
fs.writeFileSync(path.join(iconsDir, 'icon.ico'), icoBuffer);
console.log('Generated icon.ico');

// For ICNS we'll create a minimal version with the PNG data
// macOS ICNS format with ic10 (1024px) or ic07 (128px) etc.
function createIcns(pngData512) {
  // Minimal ICNS with ic09 (512x512 PNG)
  const type = Buffer.from('ic09'); // 512x512 PNG
  const iconSize = 8 + pngData512.length; // type(4) + size(4) + data
  const totalSize = 8 + iconSize; // 'icns'(4) + fileSize(4) + icon

  const buf = Buffer.alloc(totalSize);
  buf.write('icns', 0);
  buf.writeUInt32BE(totalSize, 4);
  type.copy(buf, 8);
  buf.writeUInt32BE(iconSize, 12);
  pngData512.copy(buf, 16);

  return buf;
}

// Get 512px PNG for ICNS
const resvg512 = new Resvg(svgData, { fitTo: { mode: 'width', value: 512 } });
const png512 = Buffer.from(resvg512.render().asPng());
const icnsBuffer = createIcns(png512);
fs.writeFileSync(path.join(iconsDir, 'icon.icns'), icnsBuffer);
console.log('Generated icon.icns');

console.log('\nAll icons generated successfully!');
