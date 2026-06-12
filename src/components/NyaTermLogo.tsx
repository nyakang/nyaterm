import type React from "react";
import { useId } from "react";

interface NyaTermLogoProps extends React.SVGProps<SVGSVGElement> {
  className?: string;
}

export default function NyaTermLogo({ className, ...props }: NyaTermLogoProps) {
  const id = useId().replace(/:/g, "");
  const gradientId = `nya-gradient-${id}`;
  const maskId = `eye-cutouts-${id}`;

  return (
    <svg
      width="256"
      height="256"
      viewBox="0 0 1024 1024"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      {...props}
    >
      <defs>
        <linearGradient id={gradientId} x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="var(--grad-from, var(--df-primary, #06b6d4))" />
          <stop offset="100%" stopColor="var(--grad-to, var(--df-primary-hover, #2563eb))" />
        </linearGradient>

        <mask id={maskId} maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
          <rect x="0" y="0" width="1024" height="1024" fill="white" />
          <circle cx="420" cy="558" r="44" fill="black" />
          <circle cx="604" cy="558" r="44" fill="black" />
        </mask>
      </defs>

      <rect x="0" y="0" width="1024" height="1024" rx="192" ry="192" fill={`url(#${gradientId})`} />

      <path
        fill="#FFFFFF"
        mask={`url(#${maskId})`}
        d="M512 826
          C662 826 786 724 808 590
          C816 541 809 491 788 448
          C773 417 765 385 765 350
          C765 307 767 267 753 226
          C745 202 727 187 704 183
          C676 178 649 193 620 220
          L560 278
          C546 292 529 299 512 299
          C495 299 478 292 464 278
          L404 220
          C375 193 348 178 320 183
          C297 187 279 202 271 226
          C257 267 259 307 259 350
          C259 385 251 417 236 448
          C215 491 208 541 216 590
          C238 724 362 826 512 826
          Z"
      />
    </svg>
  );
}
