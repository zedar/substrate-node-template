#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

// dev_mode relax some restrictions placed on production pallets, such as no need to specify a
// weight on every `#[pallet::call]` Note: remove dev_mode before deploying in a production runtime.
#[frame_support::pallet(dev_mode)]
pub mod pallet {
	use frame_support::{
		pallet_prelude::*,
		sp_runtime::DispatchResult,
		sp_std,
		traits::{Currency, Randomness},
	};
	use frame_system::pallet_prelude::*;
	use sp_std::prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	// Defines generic data types that the pallet uses
	// By including the Currency and Randomness interfaces the pallet will be able to:
	// 	- access and manipulate user accounts and balances
	// 	- generate on-chain randomness
	// 	- set the limit on the number of newsletters a single user can subscribe to
	// frame_system|frame_support types: AccountId, BlockNumber, Hash
	#[pallet::config]
	pub trait Config: frame_system::Config {
		// the type used accross the runtime for emitting events
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		// the interface used to manage user balances
		type Currency: Currency<Self::AccountId>;

		// the interface used to get random values
		// mapping of a feed hash to the block number
		type NewsFeedRandomness: Randomness<Self::Hash, Self::BlockNumber>;

		// max number of created feeds. Each feed may have multiple subscriptions.
		#[pallet::constant]
		type MaxNewsFeeds: Get<u32>;

		// max number of owned subscriptions
		#[pallet::constant]
		type MaxOwnedNewsFeeds: Get<u32>;

		// max number of subscriptions per account
		#[pallet::constant]
		type MaxOwnedSubscriptions: Get<u32>;

		// max length of the subscription registration url
		#[pallet::constant]
		type MaxRegistrationUrlLength: Get<u32>;
	}

	// Payment type that can be either one time or recurrent
	#[derive(Clone, Copy, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	pub enum PaymentType {
		OneTime,
		Recurrent,
	}

	// Allows easy access to the pallet's Balance type.
	type BalanceOf<T> =
		<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	// Unique id of objects created by this pallet, e.g. news feed, subscription
	type UniqueId = [u8; 16];

	// Encode - serialization of a custom type to bytes
	// Decode - deserialization of bytes into a custom type
	// TypeInfo - information of a custom type to the runtime metodata
	// MaxEncodedLen - ensures that types in the runtime are bounded in size
	// RuntimeDebug - allow a type to be printed to the console

	// TypeInfo macro forces to parse the underlying object into some JSON type. The Newseed type is
	// generic over T, we don't want to include it in the TypeInfo generation step. That's why
	// skip_type_params(T) is needed
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct NewsFeed<T: Config> {
		// unique identifier of the news feed
		pub id: UniqueId,
		// owner of the subscription
		pub owner: T::AccountId,
		// price for the subscription. None assumes not for sale/use.
		pub fee: Option<BalanceOf<T>>,
		// payment type
		pub payment_type: PaymentType,
		// url of where to register/revoke a subscription
		// Vec<u8> represents a string in substrate
		// pub registration_url: Vec<u8>,
		pub registration_url: BoundedVec<u8, T::MaxRegistrationUrlLength>,
	}

	// Represents account subscription of the news feed
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct Subscription<T: Config> {
		// unique identifier of the news feed subscription
		pub id: UniqueId,
		// unique identifier of the subscribed news feed
		pub newsfeed_id: UniqueId,
		// subscription owner
		pub owner: T::AccountId,
	}

	// Stores single value that is a number of  available news feeds.
	// The value is incremented each time news feed is created.
	// ValueQuery - if there is no value, the zero value is returned,
	// OptionQuery - None is returned,
	// ResultQuery - Err is returned.
	#[pallet::storage]
	pub(super) type NewsFeedCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	// Stores a map of feeds (each with unique_id) to their properties
	// The Twox64Concat is hashing algorithm used to store the map value.
	#[pallet::storage]
	pub(super) type NewsFeeds<T: Config> = StorageMap<_, Twox64Concat, UniqueId, NewsFeed<T>>;

	// Tracks who owns what news feeds
	#[pallet::storage]
	pub(super) type OwnerOfNewsFeeds<T: Config> = StorageMap<
		_,
		Twox64Concat,
		T::AccountId,
		BoundedVec<UniqueId, T::MaxOwnedNewsFeeds>,
		ValueQuery,
	>;

	// Tracks the news feeds subscribed by each account
	#[pallet::storage]
	pub(super) type Subscriptions<T: Config> = StorageMap<
		_,
		Twox64Concat,
		T::AccountId,
		BoundedVec<Subscription<T>, T::MaxOwnedSubscriptions>,
		ValueQuery,
	>;

	// create_subscription creates new unique subscription
	// - create unique id
	// - ensure that total number of subscriptions does not exceed the maximum allowed

	#[pallet::error]
	pub enum Error<T> {
		// each feed must have a unique identifier
		DuplicatedNewsFeed,
		// the total supply of feeds can't exceed `Config::MaxFeeds`
		NewsFeedsOverflow,
		// exceeded maximum of news feed assigned to an owner
		MaxNewsFeedsOwned,
		// news feed not found
		NewsFeedNotFound,
		// owner and target are the same
		TransferToSelf,
		// a feed url can't exceed `Config::MaxRegistrationUrlLength`
		RegistrationUrlTooLong,
		// an account must subscribe only once to the feed
		DuplicatedSubscription,
		// an account can't exceed `Config::MaxSubscriptions`
		MaxSubscriptionsExceeded,
	}

	// generate_deposit generates a helper function on Pallet that handles event depositing/sending
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		// runtime sends FeedCreated event when new feed has been successfully created
		NewsFeedCreated { feed: UniqueId, owner: T::AccountId },
	}

	// Callable functions
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		// Create a new/unique feed.
		#[pallet::weight(0)]
		pub fn create_newsfeed(
			origin: OriginFor<T>,
			payment: PaymentType,
			url: Vec<u8>,
		) -> DispatchResult {
			// Make sure the caller is from the signed origin
			let sender = ensure_signed(origin)?;

			// Generate unique id for new feed
			let newsfeed_id = Self::new_unique_id();

			// Min new feed
			Self::mint_newsfeed(&sender, newsfeed_id, payment, url.as_slice())?;

			Ok(())
		}
	}

	// Pallet internal functions
	impl<T: Config> Pallet<T> {
		// generate unique id for a new feed
		fn new_unique_id() -> UniqueId {
			// create randomness
			let random = T::NewsFeedRandomness::random(&b"unique_id"[..]).0;
			// randomness payload enables that multiple feeds can be created in the same block
			let unique_payload = (
				random,
				frame_system::Pallet::<T>::extrinsic_index().unwrap_or_default(),
				frame_system::Pallet::<T>::block_number(),
			);
			let encoded_payload = unique_payload.encode();
			frame_support::Hashable::blake2_128(&encoded_payload)
		}

		// If the function needs ownership, you should pass by value. If the function only needs a reference, you should pass by reference.
		// Mint/create new feed. Minting means creating a new news feed on the blockchain.
		// UniqueId is a fixed size array of u8 so it automatically implements Copy and Clone
		// traits. So it is passed by value == copy.
		fn mint_newsfeed(
			owner: &T::AccountId,
			newsfeed_id: UniqueId,
			payment: PaymentType,
			url: &[u8],
		) -> DispatchResult {
			// create new feed
			let feed = NewsFeed::<T> {
				id: newsfeed_id,
				owner: owner.clone(),
				fee: None,
				payment_type: payment,
				registration_url: BoundedVec::<u8, T::MaxRegistrationUrlLength>::try_from(
					url.to_vec(),
				)
				.map_err(|_| Error::<T>::RegistrationUrlTooLong)?,
			};

			// check that feed does not exist in the storage map
			ensure!(!NewsFeeds::<T>::contains_key(&feed.id), Error::<T>::DuplicatedNewsFeed);

			// check that a new feed does not exceed max available feeds
			let count = NewsFeedCount::<T>::get();
			let new_count = count.checked_add(1).ok_or(Error::<T>::NewsFeedsOverflow)?;
			ensure!(new_count <= T::MaxNewsFeeds::get(), Error::<T>::NewsFeedsOverflow);

			// let mut owned_newsfeeds = OwnerOfNewsFeeds::<T>::get(owner);
			OwnerOfNewsFeeds::<T>::try_append(owner, newsfeed_id)
				.map_err(|_| Error::<T>::MaxNewsFeedsOwned)?;

			// write new feed to the storage and update the count
			NewsFeeds::<T>::insert(feed.id, feed);
			NewsFeedCount::<T>::put(new_count);

			// deposit the event
			Self::deposit_event(Event::<T>::NewsFeedCreated {
				feed: newsfeed_id,
				owner: owner.clone(),
			});

			Ok(())
		}

		// Transfer news feed from a current owner to the new owner
		// - News feed must exists (must be registered)
		// - News feed can't be tranferred to its owner
		// - News feed can't be transferred to an account that already has the maximum news feeds
		// allowed
		fn do_newsfeed_transfer(newsfeed_id: UniqueId, to: T::AccountId) -> DispatchResult {
			// get the news feed
			let mut newsfeed =
				NewsFeeds::<T>::get(&newsfeed_id).ok_or(Error::<T>::NewsFeedNotFound)?;

			let from = newsfeed.owner;

			ensure!(from != to, Error::<T>::TransferToSelf);

			// get news feeds owned by

			Ok(())
		}
	}
}
